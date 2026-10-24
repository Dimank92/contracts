#![cfg(test)]

extern crate std;

use crate::types::DataKey as PoolKey;
use crate::{Error, LpPool, LpPoolClient, MIN_DEPOSIT, VIRTUAL_SHARES};
use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    token, Address, Env, IntoVal,
};

// ── Mock options market ──────────────────────────────────────────────────────
//
// Mirrors options_market's token flows: `write_option` pulls exactly the
// collateral from the writer and pays the premium out; `reclaim_collateral`
// returns whatever the position is worth after settlement.

#[contracttype]
enum MockKey {
    Token,
    Premium,
    NextId,
    Payout(u64),
}

#[contract]
pub struct MockMarket;

#[contractimpl]
impl MockMarket {
    pub fn init(env: Env, token: Address, premium: i128) {
        env.storage().instance().set(&MockKey::Token, &token);
        env.storage().instance().set(&MockKey::Premium, &premium);
    }

    pub fn write_option(
        env: Env,
        writer: Address,
        _series_id: u64,
        _contracts: i128,
        collateral_amount: i128,
        _min_premium: i128,
    ) -> u64 {
        writer.require_auth();
        let usdc = token::Client::new(
            &env,
            &env.storage().instance().get(&MockKey::Token).unwrap(),
        );
        usdc.transfer(&writer, &env.current_contract_address(), &collateral_amount);
        let premium: i128 = env.storage().instance().get(&MockKey::Premium).unwrap();
        usdc.transfer(&env.current_contract_address(), &writer, &premium);
        let id: u64 = env.storage().instance().get(&MockKey::NextId).unwrap_or(0);
        env.storage().instance().set(&MockKey::NextId, &(id + 1));
        env.storage()
            .instance()
            .set(&MockKey::Payout(id), &collateral_amount);
        id
    }

    /// Simulates settlement: the writer gets `payout` back instead of the
    /// full collateral.
    pub fn settle(env: Env, position_id: u64, payout: i128) {
        env.storage()
            .instance()
            .set(&MockKey::Payout(position_id), &payout);
    }

    pub fn reclaim_collateral(env: Env, writer: Address, position_id: u64) {
        writer.require_auth();
        let usdc = token::Client::new(
            &env,
            &env.storage().instance().get(&MockKey::Token).unwrap(),
        );
        let payout: i128 = env
            .storage()
            .instance()
            .get(&MockKey::Payout(position_id))
            .unwrap();
        usdc.transfer(&env.current_contract_address(), &writer, &payout);
    }
}

struct Harness<'a> {
    env: Env,
    pool: LpPoolClient<'a>,
    market: MockMarketClient<'a>,
    usdc: token::StellarAssetClient<'a>,
    token: token::Client<'a>,
    keeper: Address,
}

impl Harness<'_> {
    /// Calls `write_option` with only the keeper's signature mocked, so the
    /// market's nested `transfer(pool, market, collateral)` must be covered
    /// by the pool's own `authorize_as_current_contract`.
    fn write(&self, series_id: u64, contracts: i128, collateral: i128) -> u64 {
        self.try_write(series_id, contracts, collateral)
            .unwrap_or_else(|e| panic!("write_option failed: {e:?}"))
    }

    /// `write`, returning the pool's typed error instead of panicking.
    fn try_write(&self, series_id: u64, contracts: i128, collateral: i128) -> Result<u64, Error> {
        let args = (series_id, contracts, collateral, 0_i128);
        self.env.mock_auths(&[MockAuth {
            address: &self.keeper,
            invoke: &MockAuthInvoke {
                contract: &self.pool.address,
                fn_name: "write_option",
                args: args.into_val(&self.env),
                sub_invokes: &[],
            },
        }]);
        let result = self
            .pool
            .try_write_option(&series_id, &contracts, &collateral, &0);
        self.env.mock_all_auths();
        match result {
            Ok(id) => Ok(id.unwrap()),
            Err(Ok(e)) => Err(e),
            Err(Err(e)) => panic!("write_option failed in the host: {e:?}"),
        }
    }
}

const PREMIUM: i128 = 50_000;

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let usdc_id = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let usdc = token::StellarAssetClient::new(&env, &usdc_id);
    let market = MockMarketClient::new(&env, &env.register_contract(None, MockMarket));
    market.init(&usdc_id, &PREMIUM);
    usdc.mint(&market.address, &1_000_000_000);

    let pool = LpPoolClient::new(&env, &env.register_contract(None, LpPool));
    let keeper = Address::generate(&env);
    pool.initialize(&admin, &keeper, &usdc_id, &market.address, &8_000);
    pool.set_series_cap(&1, &10_000_000);
    Harness {
        token: token::Client::new(&env, &usdc_id),
        env,
        pool,
        market,
        usdc,
        keeper,
    }
}

fn user(h: &Harness, balance: i128) -> Address {
    let u = Address::generate(&h.env);
    h.usdc.mint(&u, &balance);
    u
}

#[test]
fn deposits_queue_until_the_epoch_is_processed() {
    let h = setup();
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
    assert_eq!(h.pool.shares_of(&alice), 0);
    assert_eq!(h.pool.total_assets(), 0);
    assert_eq!(h.token.balance(&h.pool.address), 1_000_000);

    h.pool.process_epoch();
    let (shares, _) = h.pool.claim(&alice);
    assert_eq!(shares, 1_000_000 * VIRTUAL_SHARES);
    assert_eq!(h.pool.shares_of(&alice), shares);
    assert_eq!(h.pool.total_assets(), 1_000_000);
    assert_eq!(h.pool.get_epoch(), 1);
}

#[test]
fn premiums_raise_the_share_price_across_epochs() {
    let h = setup();
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
    h.pool.process_epoch();
    h.pool.claim(&alice);

    // Epoch 1: write, settle out of the money, reclaim everything.
    let pos = h.write(1, 10, 500_000);
    assert_eq!(h.pool.idle(), 1_000_000 - 500_000 + PREMIUM);
    assert_eq!(h.pool.locked(), 500_000);
    // Bob deposits mid-epoch; he must not share in this epoch's premium.
    let bob = user(&h, 1_000_000);
    h.pool.deposit(&bob, &1_000_000);
    assert_eq!(h.pool.reclaim(&pos), 500_000);
    h.pool.process_epoch();
    h.pool.claim(&bob);

    let alice_shares = h.pool.shares_of(&alice);
    let bob_shares = h.pool.shares_of(&bob);
    assert!(bob_shares < alice_shares);

    // Both redeem at epoch 2's price.
    h.pool.request_withdraw(&alice, &alice_shares);
    h.pool.request_withdraw(&bob, &bob_shares);
    h.pool.process_epoch();
    let (_, alice_out) = h.pool.claim(&alice);
    let (_, bob_out) = h.pool.claim(&bob);
    assert!(alice_out > 1_000_000 + PREMIUM - 10);
    assert!(alice_out <= 1_000_000 + PREMIUM);
    assert!(bob_out <= 1_000_000 && bob_out > 1_000_000 - 10);
}

#[test]
fn losses_beyond_premium_reduce_the_share_price() {
    let h = setup();
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
    h.pool.process_epoch();
    h.pool.claim(&alice);
    let price_before = h.pool.share_price();

    let pos = h.write(1, 10, 500_000);
    // Keeper marks the expected payout mid-epoch.
    h.pool.set_expected_liabilities(&300_000);
    assert_eq!(h.pool.total_assets(), 1_000_000 + PREMIUM - 300_000);
    // In the money: only 200k of the 500k collateral comes back.
    h.market.settle(&pos, &200_000);
    assert_eq!(h.pool.reclaim(&pos), 200_000);
    h.pool.process_epoch();

    assert_eq!(h.pool.total_assets(), 1_000_000 + PREMIUM - 300_000);
    assert!(h.pool.share_price() < price_before);

    let shares = h.pool.shares_of(&alice);
    h.pool.request_withdraw(&alice, &shares);
    h.pool.process_epoch();
    let (_, out) = h.pool.claim(&alice);
    assert!(out <= 750_000 && out > 750_000 - 10);
}

#[test]
fn withdraw_requested_mid_epoch_waits_for_the_epoch_to_close() {
    let h = setup();
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
    h.pool.process_epoch();
    h.pool.claim(&alice);
    let shares = h.pool.shares_of(&alice);

    h.write(1, 10, 100_000);
    h.pool.request_withdraw(&alice, &shares);
    assert_eq!(h.pool.shares_of(&alice), 0);
    // Nothing to claim yet; the epoch is still open.
    assert_eq!(h.pool.claim(&alice), (0, 0));
    assert_eq!(h.token.balance(&alice), 0);
}

#[test]
fn epoch_cannot_close_with_open_positions() {
    let h = setup();
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
    h.pool.process_epoch();
    h.write(1, 10, 100_000);
    assert_eq!(
        h.pool.try_process_epoch(),
        Err(Ok(Error::PositionsStillOpen))
    );
}

#[test]
fn first_depositor_inflation_attack_is_neutralized() {
    let h = setup();
    let attacker = user(&h, 100_000_000);
    h.pool.deposit(&attacker, &MIN_DEPOSIT);
    h.pool.process_epoch();
    h.pool.claim(&attacker);

    // Donation straight to the pool's token balance.
    h.token
        .transfer(&attacker, &h.pool.address, &(100_000_000 - MIN_DEPOSIT));
    assert_eq!(h.pool.total_assets(), MIN_DEPOSIT);

    let victim = user(&h, 1_000_000);
    h.pool.deposit(&victim, &1_000_000);
    h.pool.process_epoch();
    let (victim_shares, _) = h.pool.claim(&victim);
    assert!(victim_shares > 0);

    h.pool.request_withdraw(&victim, &victim_shares);
    h.pool.process_epoch();
    let (_, out) = h.pool.claim(&victim);
    assert!(out > 1_000_000 - 10);
}

#[test]
fn dust_deposits_are_rejected() {
    let h = setup();
    let u = user(&h, 10);
    assert_eq!(
        h.pool.try_deposit(&u, &1),
        Err(Ok(Error::BelowMinimumDeposit))
    );
}

#[test]
fn writes_only_into_whitelisted_series() {
    let h = setup();
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.pool.process_epoch();
    assert_eq!(h.try_write(2, 1, 1_000), Err(Error::SeriesNotWhitelisted));
}

#[test]
fn per_series_cap_is_enforced() {
    let h = setup();
    h.pool.set_series_cap(&1, &150_000);
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.pool.process_epoch();
    h.write(1, 1, 100_000);
    assert_eq!(h.try_write(1, 1, 60_000), Err(Error::SeriesCapExceeded));
}

#[test]
fn utilization_cap_is_enforced() {
    let h = setup();
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.pool.process_epoch();
    assert_eq!(
        h.try_write(1, 1, 800_001),
        Err(Error::UtilizationCapExceeded)
    );
}

#[test]
fn pending_deposits_cannot_be_used_as_collateral() {
    let h = setup();
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    assert_eq!(h.try_write(1, 1, 1_000), Err(Error::InsufficientIdle));
}

#[test]
fn cannot_withdraw_more_shares_than_owned() {
    let h = setup();
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.pool.process_epoch();
    assert_eq!(
        h.pool
            .try_request_withdraw(&u, &(1_000_000 * VIRTUAL_SHARES + 1)),
        Err(Ok(Error::InsufficientShares))
    );
}

#[test]
fn reinitialize_and_unknown_positions_are_typed_errors() {
    let h = setup();
    let a = Address::generate(&h.env);
    assert_eq!(
        h.pool.try_initialize(&a, &a, &a, &a, &8_000),
        Err(Ok(Error::AlreadyInitialized))
    );
    assert_eq!(h.pool.try_reclaim(&42), Err(Ok(Error::PositionNotFound)));
    assert_eq!(
        h.pool.try_set_max_utilization(&0),
        Err(Ok(Error::InvalidUtilization))
    );
}

#[test]
fn uninitialized_pool_reports_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let pool = LpPoolClient::new(&env, &env.register_contract(None, LpPool));
    assert_eq!(
        pool.try_set_keeper(&Address::generate(&env)),
        Err(Ok(Error::NotInitialized))
    );
    assert_eq!(
        pool.try_deposit(&Address::generate(&env), &MIN_DEPOSIT),
        Err(Ok(Error::NotInitialized))
    );
}

#[test]
fn queued_tickets_accumulate_within_an_epoch() {
    let h = setup();
    let u = user(&h, 2_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.pool.deposit(&u, &1_000_000);
    assert_eq!(h.pool.get_deposit_ticket(&u).unwrap().amount, 2_000_000);
    h.pool.process_epoch();
    // Depositing again claims the processed ticket first.
    h.usdc.mint(&u, &MIN_DEPOSIT);
    h.pool.deposit(&u, &MIN_DEPOSIT);
    assert_eq!(h.pool.shares_of(&u), 2_000_000 * VIRTUAL_SHARES);
    assert_eq!(h.pool.get_deposit_ticket(&u).unwrap().epoch, 1);
}

#[test]
fn internal_accounting_matches_token_balance() {
    let h = setup();
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.pool.process_epoch();
    h.pool.claim(&u);
    let pos = h.write(1, 1, 300_000);
    h.market.settle(&pos, &250_000);
    h.pool.reclaim(&pos);
    let shares = h.pool.shares_of(&u);
    h.pool.request_withdraw(&u, &(shares / 2));
    h.pool.process_epoch();
    let (_, out) = h.pool.claim(&u);
    assert!(out > 0);
    let claimable: i128 = h.env.as_contract(&h.pool.address, || {
        h.env
            .storage()
            .instance()
            .get(&PoolKey::Claimable)
            .unwrap_or(0)
    });
    assert_eq!(
        h.token.balance(&h.pool.address),
        h.pool.idle() + h.pool.locked() + claimable
    );
}
