//! Governance handover (docs/governance-handover.md), end to end:
//!
//!   Stage 0  deployer EOA is admin of options_market and price_oracle
//!   Stage 1  admin → multisig account
//!   Stage 2  admin → timelock (proposer + guardian = multisig)
//!   Stage 3  timelock proposer → governor (guardian still multisig)
//!   Stage 4  lock-in: guardian removed, no more rollback
//!
//! Admin transfers are two-step: the current admin proposes, the new admin
//! calls `accept_admin` — for the timelock, through one of its own delayed
//! operations. The vault's admin stays options_market throughout.
//!
//! Auth is never mocked wholesale after setup: every privileged call
//! supplies exactly one signer's authorization, so "X can do it and Y can
//! not" is actually enforced by the host at every stage.

use governor::{Governor, GovernorClient, ProposalState};
use multisig::{Multisig, MultisigClient};
use options_market::{OptionsMarket, OptionsMarketClient};
use price_oracle::{PriceOracle, PriceOracleClient};
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short,
    testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
    vec, Address, BytesN, Env, IntoVal, Symbol, Val, Vec,
};
use timelock::{Timelock, TimelockClient};
use vault::{Vault, VaultClient};

const TL_DELAY: u64 = 2 * 86_400;
const TL_GRACE: u64 = 7 * 86_400;
const VOTING_DELAY: u32 = 10;
const VOTING_PERIOD: u32 = 720;

// ── Minimal checkpointed voting token (stand-in for gov_token) ──────────────

#[contracttype]
enum VotesKey {
    Account(Address),
    Supply,
}

#[contract]
struct MockVotes;

fn past(cps: Option<Vec<(u32, i128)>>, ledger: u32) -> i128 {
    let mut value = 0;
    for (at, v) in cps.into_iter().flatten() {
        if at <= ledger {
            value = v;
        }
    }
    value
}

#[contractimpl]
impl MockVotes {
    pub fn mint(env: Env, to: Address, amount: i128) {
        for key in [VotesKey::Account(to), VotesKey::Supply] {
            let mut cps: Vec<(u32, i128)> =
                env.storage().instance().get(&key).unwrap_or(vec![&env]);
            let last = cps.last().map(|(_, v)| v).unwrap_or(0);
            cps.push_back((env.ledger().sequence(), last + amount));
            env.storage().instance().set(&key, &cps);
        }
    }
    pub fn get_past_votes(env: Env, account: Address, ledger: u32) -> i128 {
        past(
            env.storage().instance().get(&VotesKey::Account(account)),
            ledger,
        )
    }
    pub fn get_past_total_supply(env: Env, ledger: u32) -> i128 {
        past(env.storage().instance().get(&VotesKey::Supply), ledger)
    }
}

// ── Harness ─────────────────────────────────────────────────────────────────

struct World<'a> {
    env: Env,
    deployer: Address,
    /// The native Stellar multisig account (M-of-N signers) that holds
    /// admin during stage 1 and acts as timelock proposer/guardian.
    ms_account: Address,
    ms_signers: [Address; 3],
    /// The on-chain Multisig contract backing every `*_via_multisig`
    /// recovery path, including the vault's.
    multisig: MultisigClient<'a>,
    market: OptionsMarketClient<'a>,
    oracle: PriceOracleClient<'a>,
    vault: VaultClient<'a>,
    tl: TimelockClient<'a>,
    gov: GovernorClient<'a>,
    voter: Address,
    op_nonce: core::cell::Cell<u8>,
}

fn deploy<'a>() -> World<'a> {
    let env = Env::default();
    env.ledger().set_sequence_number(100);
    env.ledger().set_timestamp(1_000_000);
    env.mock_all_auths();

    let deployer = Address::generate(&env);
    let ms_account = Address::generate(&env);
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

    let usdc = env
        .register_stellar_asset_contract_v2(deployer.clone())
        .address();
    let oracle = PriceOracleClient::new(&env, &env.register_contract(None, PriceOracle));
    oracle.initialize(&deployer);
    let market = OptionsMarketClient::new(&env, &env.register_contract(None, OptionsMarket));
    market.initialize(&deployer, &oracle.address, &usdc, &deployer);
    // The vault trusts the multisig for its recovery path, then hands its
    // admin to options_market (which accepts it, two-step).
    let vault = VaultClient::new(&env, &env.register_contract(None, Vault));
    vault.initialize(&deployer, &usdc);
    vault.set_multisig(&multisig.address);
    vault.transfer_admin(&market.address);
    market.accept_vault_admin(&vault.address);

    let votes = MockVotesClient::new(&env, &env.register_contract(None, MockVotes));
    let voter = Address::generate(&env);
    votes.mint(&voter, &1_000_000);

    let tl = TimelockClient::new(&env, &env.register_contract(None, Timelock));
    tl.initialize(&ms_account, &ms_account, &TL_DELAY, &TL_GRACE);
    let gov = GovernorClient::new(&env, &env.register_contract(None, Governor));
    gov.initialize(
        &votes.address,
        &tl.address,
        &VOTING_DELAY,
        &VOTING_PERIOD,
        &400,
        &1_000,
    );

    // From here on, nothing is authorized unless a test says who signs.
    env.set_auths(&[]);
    let w = World {
        env,
        deployer,
        ms_account,
        ms_signers,
        multisig,
        market,
        oracle,
        vault,
        tl,
        gov,
        voter,
        op_nonce: core::cell::Cell::new(0),
    };
    w.roll(1);
    w
}

impl World<'_> {
    fn roll(&self, ledgers: u32) {
        let seq = self.env.ledger().sequence();
        self.env.ledger().set_sequence_number(seq + ledgers);
    }

    fn warp(&self, secs: u64) {
        let t = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(t + secs);
    }

    /// Authorizes exactly one call to `contract.fn_name(args)` by `who`.
    fn sign(&self, who: &Address, contract: &Address, fn_name: &str, args: Vec<Val>) {
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

    /// Can `who` exercise every admin function we check on both contracts?
    fn admin_can_act(&self, who: &Address) -> bool {
        let e = &self.env;
        self.sign(
            who,
            &self.market.address,
            "set_fee_rate",
            (75u32,).into_val(e),
        );
        let fee_ok = self.market.try_set_fee_rate(&75).is_ok();
        self.sign(
            who,
            &self.oracle.address,
            "set_max_staleness",
            (600u64,).into_val(e),
        );
        let oracle_ok = self.oracle.try_set_max_staleness(&600).is_ok();
        self.sign(who, &self.market.address, "pause", ().into_val(e));
        let pause_ok = self.market.try_pause().is_ok();
        if pause_ok {
            self.sign(who, &self.market.address, "unpause", ().into_val(e));
            self.market.unpause();
        }
        assert_eq!(fee_ok, oracle_ok);
        assert_eq!(fee_ok, pause_ok);
        fee_ok
    }

    fn assert_vault_invariant(&self) {
        assert_eq!(self.vault.get_admin(), self.market.address);
    }

    /// `who` (an account) calls transfer_admin(new) on both contracts:
    /// step one of the two-step transfer.
    fn transfer_both(&self, who: &Address, new_admin: &Address) {
        let args: Vec<Val> = (new_admin.clone(),).into_val(&self.env);
        self.sign(who, &self.market.address, "transfer_admin", args.clone());
        self.market.transfer_admin(new_admin);
        self.sign(who, &self.oracle.address, "transfer_admin", args);
        self.oracle.transfer_admin(new_admin);
    }

    /// `who` (an account) accepts a pending admin transfer on both.
    fn accept_both(&self, who: &Address) {
        let none: Vec<Val> = ().into_val(&self.env);
        self.sign(who, &self.market.address, "accept_admin", none.clone());
        self.market.accept_admin();
        self.sign(who, &self.oracle.address, "accept_admin", none);
        self.oracle.accept_admin();
    }

    /// The timelock's own `accept_admin` calls on both contracts.
    fn accept_both_calls(&self) -> (Vec<Address>, Vec<Symbol>, Vec<Vec<Val>>) {
        let e = &self.env;
        (
            vec![e, self.market.address.clone(), self.oracle.address.clone()],
            vec![
                e,
                Symbol::new(e, "accept_admin"),
                Symbol::new(e, "accept_admin"),
            ],
            vec![e, Vec::<Val>::new(e), Vec::<Val>::new(e)],
        )
    }

    fn transfer_both_calls(
        &self,
        new_admin: &Address,
    ) -> (Vec<Address>, Vec<Symbol>, Vec<Vec<Val>>) {
        let e = &self.env;
        let arg: Vec<Val> = vec![e, new_admin.into_val(e)];
        (
            vec![e, self.market.address.clone(), self.oracle.address.clone()],
            vec![
                e,
                Symbol::new(e, "transfer_admin"),
                Symbol::new(e, "transfer_admin"),
            ],
            vec![e, arg.clone(), arg],
        )
    }

    /// Multisig (as timelock proposer) schedules a batch; returns its id.
    fn ms_schedule(&self, calls: (Vec<Address>, Vec<Symbol>, Vec<Vec<Val>>)) -> BytesN<32> {
        let n = self.op_nonce.get() + 1;
        self.op_nonce.set(n);
        let id = BytesN::from_array(&self.env, &[n; 32]);
        let (t, f, a) = calls;
        let args: Vec<Val> =
            (id.clone(), t.clone(), f.clone(), a.clone(), TL_DELAY).into_val(&self.env);
        self.sign(&self.ms_account, &self.tl.address, "schedule", args);
        self.tl.schedule(&id, &t, &f, &a, &TL_DELAY);
        id
    }

    fn execute_after_delay(&self, id: &BytesN<32>) {
        self.warp(TL_DELAY);
        self.env.set_auths(&[]);
        self.tl.execute(id);
    }

    /// Runs a full governor proposal through to timelock execution.
    fn govern(&self, calls: (Vec<Address>, Vec<Symbol>, Vec<Vec<Val>>)) -> BytesN<32> {
        let e = &self.env;
        let (t, f, a) = calls;
        let n = self.op_nonce.get() + 1;
        self.op_nonce.set(n);
        let desc = BytesN::from_array(e, &[n; 32]);
        e.mock_all_auths();
        let id = self.gov.propose(&self.voter, &t, &f, &a, &desc);
        self.roll(VOTING_DELAY + 1);
        self.gov.cast_vote(&self.voter, &id, &1);
        self.roll(VOTING_PERIOD);
        e.set_auths(&[]);
        self.gov.queue(&id);
        self.execute_after_delay(&id);
        assert_eq!(self.gov.state(&id), ProposalState::Executed);
        id
    }

    fn to_stage1(&self) {
        self.transfer_both(&self.deployer.clone(), &self.ms_account.clone());
        self.accept_both(&self.ms_account.clone());
    }

    /// The multisig proposes the timelock, then (as timelock proposer)
    /// schedules the timelock's acceptance.
    fn stage1_to_stage2(&self) {
        self.transfer_both(&self.ms_account.clone(), &self.tl.address.clone());
        let id = self.ms_schedule(self.accept_both_calls());
        self.execute_after_delay(&id);
    }

    fn to_stage2(&self) {
        self.to_stage1();
        self.stage1_to_stage2();
    }

    fn to_stage3(&self) {
        self.to_stage2();
        let e = &self.env;
        let id = self.ms_schedule((
            vec![e, self.tl.address.clone()],
            vec![e, symbol_short!("set_prop")],
            vec![e, vec![e, self.gov.address.into_val(e)]],
        ));
        self.execute_after_delay(&id);
    }

    fn to_stage4(&self) {
        self.to_stage3();
        let e = &self.env;
        self.govern((
            vec![e, self.tl.address.clone()],
            vec![e, symbol_short!("lock_in")],
            vec![e, Vec::<Val>::new(e)],
        ));
    }
}

// ── Full staged handover ────────────────────────────────────────────────────

#[test]
fn staged_handover_end_to_end() {
    let w = deploy();

    // Stage 0
    assert!(w.admin_can_act(&w.deployer));
    assert!(!w.admin_can_act(&w.ms_account));
    w.assert_vault_invariant();

    // Stage 1
    w.to_stage1();
    assert_eq!(w.market.get_admin(), w.ms_account);
    assert_eq!(w.oracle.get_admin(), w.ms_account);
    assert!(w.admin_can_act(&w.ms_account));
    assert!(!w.admin_can_act(&w.deployer));
    w.assert_vault_invariant();

    // Stage 2: proposing the timelock moves nothing by itself…
    w.transfer_both(&w.ms_account, &w.tl.address);
    assert!(w.admin_can_act(&w.ms_account));
    // …the timelock's own delayed accept_admin does.
    let accept = w.ms_schedule(w.accept_both_calls());
    w.execute_after_delay(&accept);
    assert_eq!(w.market.get_admin(), w.tl.address);
    assert!(!w.admin_can_act(&w.ms_account));
    assert!(!w.admin_can_act(&w.deployer));
    let e = &w.env;
    let id = w.ms_schedule((
        vec![e, w.market.address.clone()],
        vec![e, Symbol::new(e, "set_fee_rate")],
        vec![e, vec![e, 120u32.into_val(e)]],
    ));
    w.execute_after_delay(&id);
    assert_eq!(w.market.get_fee_rate(), 120);
    w.assert_vault_invariant();

    // Stage 3
    let id = w.ms_schedule((
        vec![e, w.tl.address.clone()],
        vec![e, symbol_short!("set_prop")],
        vec![e, vec![e, w.gov.address.into_val(e)]],
    ));
    w.execute_after_delay(&id);
    assert_eq!(w.tl.get_proposer(), w.gov.address);
    // Multisig can no longer schedule.
    let calls = w.transfer_both_calls(&w.ms_account);
    let (t, f, a) = calls.clone();
    let bogus = BytesN::from_array(e, &[0xAA; 32]);
    w.sign(
        &w.ms_account,
        &w.tl.address,
        "schedule",
        (bogus.clone(), t.clone(), f.clone(), a.clone(), TL_DELAY).into_val(e),
    );
    assert!(w.tl.try_schedule(&bogus, &t, &f, &a, &TL_DELAY).is_err());
    // Governor can act on every admin function.
    w.govern((
        vec![e, w.market.address.clone(), w.oracle.address.clone()],
        vec![
            e,
            Symbol::new(e, "set_fee_rate"),
            Symbol::new(e, "set_max_staleness"),
        ],
        vec![e, vec![e, 90u32.into_val(e)], vec![e, 900u64.into_val(e)]],
    ));
    assert_eq!(w.market.get_fee_rate(), 90);
    assert_eq!(w.oracle.get_max_staleness(), 900);
    w.assert_vault_invariant();

    // Stage 4
    w.govern((
        vec![e, w.tl.address.clone()],
        vec![e, symbol_short!("lock_in")],
        vec![e, Vec::<Val>::new(e)],
    ));
    assert!(w.tl.is_locked_in());
    assert_eq!(w.market.get_admin(), w.tl.address);
    assert_eq!(w.oracle.get_admin(), w.tl.address);
    w.assert_vault_invariant();
}

// ── Rollback at each stage ──────────────────────────────────────────────────

#[test]
fn rollback_stage1_multisig_returns_admin() {
    let w = deploy();
    w.to_stage1();
    w.transfer_both(&w.ms_account, &w.deployer);
    w.accept_both(&w.deployer);
    assert!(w.admin_can_act(&w.deployer));
    assert!(!w.admin_can_act(&w.ms_account));
}

#[test]
fn rollback_stage2_multisig_reclaims_admin_through_timelock() {
    let w = deploy();
    w.to_stage2();
    let id = w.ms_schedule(w.transfer_both_calls(&w.ms_account));
    w.execute_after_delay(&id);
    w.accept_both(&w.ms_account);
    assert!(w.admin_can_act(&w.ms_account));
    w.assert_vault_invariant();
}

#[test]
fn rollback_stage2_guardian_cancels_bad_handover() {
    let w = deploy();
    w.to_stage2();
    let attacker = Address::generate(&w.env);
    let id = w.ms_schedule(w.transfer_both_calls(&attacker));
    w.sign(
        &w.ms_account,
        &w.tl.address,
        "cancel",
        (w.ms_account.clone(), id.clone()).into_val(&w.env),
    );
    w.tl.cancel(&w.ms_account, &id);
    w.warp(TL_DELAY);
    assert!(w.tl.try_execute(&id).is_err());
    assert_eq!(w.market.get_admin(), w.tl.address);
    assert_eq!(w.market.get_pending_admin(), None);
}

#[test]
fn rollback_stage3_guardian_restores_multisig_proposer() {
    let w = deploy();
    w.to_stage3();

    // A malicious governor proposal gets queued …
    let attacker = Address::generate(&w.env);
    let (t, f, a) = w.transfer_both_calls(&attacker);
    w.env.mock_all_auths();
    let id = w.gov.propose(
        &w.voter,
        &t,
        &f,
        &a,
        &BytesN::from_array(&w.env, &[0xEE; 32]),
    );
    w.roll(VOTING_DELAY + 1);
    w.gov.cast_vote(&w.voter, &id, &1);
    w.roll(VOTING_PERIOD);
    w.env.set_auths(&[]);
    w.gov.queue(&id);

    // … the guardian vetoes it and rolls the proposer back to itself.
    w.sign(
        &w.ms_account,
        &w.tl.address,
        "cancel",
        (w.ms_account.clone(), id.clone()).into_val(&w.env),
    );
    w.tl.cancel(&w.ms_account, &id);
    w.sign(
        &w.ms_account,
        &w.tl.address,
        "guardian_set_proposer",
        (w.ms_account.clone(),).into_val(&w.env),
    );
    w.tl.guardian_set_proposer(&w.ms_account);
    assert_eq!(w.tl.get_proposer(), w.ms_account);
    w.warp(TL_DELAY);
    assert!(w.tl.try_execute(&id).is_err());
    assert_eq!(w.market.get_admin(), w.tl.address);

    // The multisig is back in control of the timelock.
    let fix = w.ms_schedule(w.transfer_both_calls(&w.ms_account));
    w.execute_after_delay(&fix);
    w.accept_both(&w.ms_account);
    assert!(w.admin_can_act(&w.ms_account));
}

#[test]
fn stage4_lock_in_removes_every_rollback() {
    let w = deploy();
    w.to_stage4();
    w.sign(
        &w.ms_account,
        &w.tl.address,
        "guardian_set_proposer",
        (w.ms_account.clone(),).into_val(&w.env),
    );
    assert!(w.tl.try_guardian_set_proposer(&w.ms_account).is_err());
    assert_eq!(w.tl.get_proposer(), w.gov.address);
    assert!(!w.admin_can_act(&w.ms_account));
    assert!(!w.admin_can_act(&w.deployer));
}

#[test]
fn vault_multisig_recovery_path_survives_every_stage() {
    let w = deploy();
    w.to_stage4();
    w.assert_vault_invariant();
    // The vault's only migration concern is its multisig recovery path:
    // with 2-of-3 signer approvals of its trusted multisig it can still
    // re-point the vault admin (two-step: the recovery address accepts).
    let action_id = BytesN::from_array(&w.env, &[0x90; 32]);
    for s in &w.ms_signers[..2] {
        w.sign(
            s,
            &w.multisig.address,
            "approve",
            (s.clone(), action_id.clone()).into_val(&w.env),
        );
        w.multisig.approve(s, &action_id);
    }
    assert!(w.multisig.is_approved(&action_id));
    let recovery = Address::generate(&w.env);
    w.env.set_auths(&[]);
    w.vault
        .transfer_admin_via_multisig(&w.multisig.address, &action_id, &recovery);
    assert_eq!(w.vault.get_admin(), w.market.address);
    w.sign(
        &recovery,
        &w.vault.address,
        "accept_admin",
        ().into_val(&w.env),
    );
    w.vault.accept_admin();
    assert_eq!(w.vault.get_admin(), recovery);
}
