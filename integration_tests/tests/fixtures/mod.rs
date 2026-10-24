//! Shared fixtures for the end-to-end suites: one call deploys and wires
//! every contract into a single `Env`, so each scenario reads as a
//! sequence of user actions instead of deployment boilerplate.
//!
//! Each test calls `deploy()` for a fresh `Env`, so no test depends on
//! another's state or on execution order. Auth is mocked with plain
//! `mock_all_auths()` — NOT the `_allowing_non_root_auth` variant — so a
//! contract that calls another contract on a user's or its own behalf
//! must authorize that nested call the way it would on-chain.
//!
//! Used from `tests/*.rs` with `mod fixtures;`.

#![allow(dead_code)]

use multisig::{Multisig, MultisigClient};
use options_market::{OptionType, OptionsMarket, OptionsMarketClient};
use price_oracle::{PriceOracle, PriceOracleClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _, MockAuth, MockAuthInvoke},
    token, vec, Address, BytesN, Env, IntoVal, Symbol, TryFromVal, Val, Vec,
};
use timelock::{Timelock, TimelockClient};
use vault::{Vault, VaultClient};

/// 1 USDC / 1 contract / price unit of 1.00 — all contracts use 7 decimals.
pub const UNIT: i128 = 10_000_000;
pub const DAY: u64 = 86_400;
pub const START_TIME: u64 = 1_700_000_000;

pub struct Protocol<'a> {
    pub env: Env,
    /// The deployer key, admin of the market and oracle after deploy.
    pub admin: Address,
    pub fee_recipient: Address,
    pub usdc: token::Client<'a>,
    pub usdc_admin: token::StellarAssetClient<'a>,
    /// 2-of-3 multisig contract, trusted by market, oracle and vault.
    pub multisig: MultisigClient<'a>,
    pub ms_signers: [Address; 3],
    pub oracle: PriceOracleClient<'a>,
    pub feeders: [Address; 3],
    /// Escrow vault; its admin is the market.
    pub vault: VaultClient<'a>,
    pub market: OptionsMarketClient<'a>,
    nonce: core::cell::Cell<u8>,
}

/// Deploys and wires the protocol:
///
/// - USDC (a Stellar Asset Contract) as the collateral token;
/// - a 2-of-3 `multisig`, pinned as every contract's trusted multisig;
/// - `price_oracle` with three feeders and `min_reports = 2`;
/// - `vault`, initialized by the deployer and handed (two-step) to the
///   market, which escrows writer collateral in it;
/// - `options_market` reading `price_oracle` for permissionless settlement.
pub fn deploy<'a>() -> Protocol<'a> {
    let env = Env::default();
    env.ledger().set_timestamp(START_TIME);
    env.ledger().set_sequence_number(1_000);
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let fee_recipient = Address::generate(&env);
    let usdc_addr = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let usdc = token::Client::new(&env, &usdc_addr);
    let usdc_admin = token::StellarAssetClient::new(&env, &usdc_addr);

    let ms_signers = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let multisig = MultisigClient::new(&env, &env.register_contract(None, Multisig));
    multisig.initialize(
        &vec![
            &env,
            ms_signers[0].clone(),
            ms_signers[1].clone(),
            ms_signers[2].clone(),
        ],
        &2,
        &0,
    );

    let oracle = PriceOracleClient::new(&env, &env.register_contract(None, PriceOracle));
    oracle.initialize(&admin);
    oracle.set_multisig(&multisig.address);
    let feeders = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    for f in &feeders {
        oracle.add_feeder(f);
    }
    oracle.set_min_reports(&2);

    let market = OptionsMarketClient::new(&env, &env.register_contract(None, OptionsMarket));
    market.initialize(&admin, &oracle.address, &usdc_addr, &fee_recipient);
    market.set_multisig(&multisig.address);

    let vault = VaultClient::new(&env, &env.register_contract(None, Vault));
    vault.initialize(&admin, &usdc_addr);
    vault.set_multisig(&multisig.address);
    vault.transfer_admin(&market.address);
    market.accept_vault_admin(&vault.address);
    market.set_vault(&vault.address);

    Protocol {
        env,
        admin,
        fee_recipient,
        usdc,
        usdc_admin,
        multisig,
        ms_signers,
        oracle,
        feeders,
        vault,
        market,
        nonce: core::cell::Cell::new(0),
    }
}

impl Protocol<'_> {
    pub fn xlm(&self) -> Symbol {
        Symbol::new(&self.env, "XLM")
    }

    /// A fresh funded account.
    pub fn user(&self, usdc: i128) -> Address {
        let who = Address::generate(&self.env);
        if usdc > 0 {
            self.usdc_admin.mint(&who, &usdc);
        }
        who
    }

    pub fn balance(&self, of: &Address) -> i128 {
        self.usdc.balance(of)
    }

    pub fn now(&self) -> u64 {
        self.env.ledger().timestamp()
    }

    pub fn warp(&self, secs: u64) {
        let t = self.now();
        self.env.ledger().set_timestamp(t + secs);
    }

    pub fn warp_to(&self, timestamp: u64) {
        self.env.ledger().set_timestamp(timestamp);
    }

    /// Lists an XLM series expiring in 30 days with 45% implied vol.
    pub fn list_series(&self, option_type: OptionType, strike: i128, premium: i128) -> u64 {
        self.market.create_series(
            &self.xlm(),
            &option_type,
            &strike,
            &(self.now() + 30 * DAY),
            &premium,
            &450_000_000,
        )
    }

    pub fn expiry(&self, series_id: u64) -> u64 {
        self.market.get_series(&series_id).unwrap().expiry
    }

    /// Each feeder in `prices` order reports `XLM` now.
    pub fn report(&self, prices: &[i128]) {
        for (feeder, price) in self.feeders.iter().zip(prices) {
            self.oracle.report_price(feeder, &self.xlm(), price);
        }
    }

    /// A unique 32-byte id.
    pub fn next_id(&self) -> BytesN<32> {
        let n = self.nonce.get() + 1;
        self.nonce.set(n);
        BytesN::from_array(&self.env, &[n; 32])
    }

    /// Two of the three multisig signers approve `id`.
    pub fn approve(&self, id: &BytesN<32>) {
        self.multisig.approve(&self.ms_signers[0], id);
        self.multisig.approve(&self.ms_signers[1], id);
        assert!(self.multisig.is_approved(id));
    }

    /// Authorizes exactly one call to `contract.fn_name(args)` by `who`,
    /// replacing any mocked auth — so "X can and Y can't" is enforced by
    /// the host.
    pub fn sign(&self, who: &Address, contract: &Address, fn_name: &str, args: Vec<Val>) {
        self.env.mock_auths(&[MockAuth {
            address: who,
            invoke: &MockAuthInvoke {
                contract,
                fn_name,
                args,
                sub_invokes: &[],
            },
        }]);
    }

    /// Topics[0] symbols of every event `contract` emitted so far.
    pub fn event_names(&self, contract: &Address) -> std::vec::Vec<std::string::String> {
        let mut names = std::vec::Vec::new();
        for (from, topics, _) in self.env.events().all().iter() {
            if from != *contract {
                continue;
            }
            if let Some(first) = topics.get(0) {
                if let Ok(sym) = Symbol::try_from_val(&self.env, &first) {
                    names.push(sym.to_string());
                }
            }
        }
        names
    }

    /// Data of the last event `contract` emitted under `name`.
    pub fn last_event_data(&self, contract: &Address, name: &str) -> Option<Val> {
        let wanted = Symbol::new(&self.env, name);
        let mut found = None;
        for (from, topics, data) in self.env.events().all().iter() {
            if from != *contract {
                continue;
            }
            let matches = topics
                .get(0)
                .and_then(|t| Symbol::try_from_val(&self.env, &t).ok())
                .is_some_and(|s| s == wanted);
            if matches {
                found = Some(data);
            }
        }
        found
    }

    // ── timelock ─────────────────────────────────────────────────────────────

    /// A timelock whose proposer and guardian are `proposer`.
    pub fn timelock<'b>(
        &self,
        proposer: &Address,
        min_delay: u64,
        grace: u64,
    ) -> TimelockClient<'b> {
        let tl = TimelockClient::new(&self.env, &self.env.register_contract(None, Timelock));
        tl.initialize(proposer, proposer, &min_delay, &grace);
        tl
    }

    /// One timelock call: (target, function, args).
    pub fn call(
        &self,
        target: &Address,
        func: &str,
        args: Vec<Val>,
    ) -> (Address, Symbol, Vec<Val>) {
        (target.clone(), Symbol::new(&self.env, func), args)
    }

    /// Schedules `calls` on `tl` as one operation, `delay` from now.
    pub fn schedule(
        &self,
        tl: &TimelockClient,
        calls: &[(Address, Symbol, Vec<Val>)],
        delay: u64,
        predecessor: Option<&BytesN<32>>,
    ) -> BytesN<32> {
        let e = &self.env;
        let mut targets = Vec::new(e);
        let mut fns = Vec::new(e);
        let mut args = Vec::new(e);
        for (t, f, a) in calls {
            targets.push_back(t.clone());
            fns.push_back(f.clone());
            args.push_back(a.clone());
        }
        let id = self.next_id();
        match predecessor {
            Some(pred) => tl.schedule_after(&id, pred, &targets, &fns, &args, &delay),
            None => tl.schedule(&id, &targets, &fns, &args, &delay),
        };
        id
    }

    /// The `accept_admin` calls the timelock must make to finish a
    /// two-step hand-off of each of `contracts` to itself.
    pub fn accept_calls(
        &self,
        contracts: &[&Address],
    ) -> std::vec::Vec<(Address, Symbol, Vec<Val>)> {
        contracts
            .iter()
            .map(|c| self.call(c, "accept_admin", Vec::new(&self.env)))
            .collect()
    }

    pub fn args1<T: IntoVal<Env, Val>>(&self, v: T) -> Vec<Val> {
        vec![&self.env, v.into_val(&self.env)]
    }
}
