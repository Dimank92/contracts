#![cfg(test)]

extern crate std;

use crate::types::DataKey as PoolKey;
use crate::{LpPool, LpPoolClient, MIN_DEPOSIT, VIRTUAL_SHARES};
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
        let id = self
            .pool
            .write_option(&series_id, &contracts, &collateral, &0);
        self.env.mock_all_auths();
        id
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
#[should_panic(expected = "Error(Contract, #9)")] // PositionsStillOpen
fn epoch_cannot_close_with_open_positions() {
    let h = setup();
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
    h.pool.process_epoch();
    h.write(1, 10, 100_000);
    h.pool.process_epoch();
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
#[should_panic(expected = "Error(Contract, #13)")] // BelowMinimumDeposit
fn dust_deposits_are_rejected() {
    let h = setup();
    let u = user(&h, 10);
    h.pool.deposit(&u, &1);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // SeriesNotWhitelisted
fn writes_only_into_whitelisted_series() {
    let h = setup();
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.pool.process_epoch();
    h.write(2, 1, 1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // SeriesCapExceeded
fn per_series_cap_is_enforced() {
    let h = setup();
    h.pool.set_series_cap(&1, &150_000);
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.pool.process_epoch();
    h.write(1, 1, 100_000);
    h.write(1, 1, 60_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // UtilizationCapExceeded
fn utilization_cap_is_enforced() {
    let h = setup();
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.pool.process_epoch();
    h.write(1, 1, 800_001);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // InsufficientIdle
fn pending_deposits_cannot_be_used_as_collateral() {
    let h = setup();
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.write(1, 1, 1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // InsufficientShares
fn cannot_withdraw_more_shares_than_owned() {
    let h = setup();
    let u = user(&h, 1_000_000);
    h.pool.deposit(&u, &1_000_000);
    h.pool.process_epoch();
    h.pool
        .request_withdraw(&u, &(1_000_000 * VIRTUAL_SHARES + 1));
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

// ── Caps, fees and pause ─────────────────────────────────────────────────────

#[test]
#[should_panic(expected = "Error(Contract, #14)")] // DepositCapExceeded
fn per_epoch_cap_rejects_a_breaching_deposit() {
    let h = setup();
    h.pool.set_deposit_caps(&2_000_000, &0);
    let alice = user(&h, 5_000_000);
    h.pool.deposit(&alice, &1_500_000);
    h.pool.deposit(&alice, &500_001);
}

#[test]
fn deposit_exactly_at_the_cap_is_accepted_and_resets_next_epoch() {
    let h = setup();
    h.pool.set_deposit_caps(&2_000_000, &0);
    let alice = user(&h, 5_000_000);
    h.pool.deposit(&alice, &1_500_000);
    h.pool.deposit(&alice, &500_000);
    h.pool.process_epoch();
    h.pool.deposit(&alice, &2_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")] // TotalCapacityExceeded
fn total_cap_counts_existing_assets_and_queued_deposits() {
    let h = setup();
    h.pool.set_deposit_caps(&0, &3_000_000);
    let alice = user(&h, 5_000_000);
    h.pool.deposit(&alice, &2_000_000);
    h.pool.process_epoch();
    h.pool.deposit(&alice, &1_000_000);
    h.pool.deposit(&alice, &1_000);
}

#[test]
fn lowering_caps_below_current_size_only_blocks_new_deposits() {
    let h = setup();
    let alice = user(&h, 5_000_000);
    h.pool.deposit(&alice, &2_000_000);
    h.pool.process_epoch();
    h.pool.claim(&alice);
    h.pool.set_deposit_caps(&0, &1_000_000);
    assert!(h.pool.try_deposit(&alice, &1_000).is_err());
    // The existing position is untouched and can still exit.
    let shares = h.pool.shares_of(&alice);
    h.pool.request_withdraw(&alice, &shares);
    h.pool.process_epoch();
    assert!(h.pool.claim(&alice).1 >= 2_000_000 - 10);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")] // InvalidFeeConfig
fn fees_over_100_percent_are_rejected() {
    let h = setup();
    let t = Address::generate(&h.env);
    h.pool.set_fees(&6_000, &4_001, &Some(t));
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")] // InvalidFeeConfig
fn nonzero_fees_require_a_treasury() {
    let h = setup();
    h.pool.set_fees(&100, &0, &None);
}

#[test]
fn performance_fee_is_paid_on_profit_to_the_treasury() {
    let h = setup();
    let treasury = Address::generate(&h.env);
    h.pool.set_fees(&1_000, &0, &Some(treasury.clone()));
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
    h.pool.process_epoch();
    assert_eq!(h.token.balance(&treasury), 0);

    let pos = h.write(1, 10, 500_000);
    h.pool.reclaim(&pos);
    h.pool.process_epoch();
    assert_eq!(h.token.balance(&treasury), PREMIUM / 10);
    assert_eq!(h.pool.idle(), 1_000_000 + PREMIUM - PREMIUM / 10);
    assert_eq!(h.token.balance(&h.pool.address), h.pool.idle());
}

#[test]
fn losing_epoch_accrues_no_performance_fee() {
    let h = setup();
    let treasury = Address::generate(&h.env);
    h.pool.set_fees(&10_000, &0, &Some(treasury.clone()));
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
    h.pool.process_epoch();

    let pos = h.write(1, 10, 500_000);
    h.market.settle(&pos, &200_000);
    h.pool.reclaim(&pos);
    h.pool.process_epoch();
    assert_eq!(h.token.balance(&treasury), 0);
}

#[test]
fn management_fee_is_a_share_of_assets_each_epoch() {
    let h = setup();
    let treasury = Address::generate(&h.env);
    h.pool.set_fees(&0, &100, &Some(treasury.clone()));
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
    h.pool.process_epoch();
    assert_eq!(h.token.balance(&treasury), 0);
    h.pool.process_epoch();
    assert_eq!(h.token.balance(&treasury), 10_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")] // Paused
fn pause_blocks_new_deposits() {
    let h = setup();
    h.pool.set_paused(&true);
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
}

#[test]
fn pause_still_lets_withdrawals_flush_and_redeem() {
    let h = setup();
    let alice = user(&h, 1_000_000);
    h.pool.deposit(&alice, &1_000_000);
    h.pool.process_epoch();
    h.pool.claim(&alice);

    h.pool.set_paused(&true);
    let shares = h.pool.shares_of(&alice);
    h.pool.request_withdraw(&alice, &shares);
    h.pool.process_epoch();
    assert!(h.pool.claim(&alice).1 > 0);

    h.pool.set_paused(&false);
    h.pool.deposit(&alice, &1_000);
}

#[test]
#[should_panic]
fn only_the_admin_can_configure() {
    let h = setup();
    h.env.mock_auths(&[]);
    h.pool.set_paused(&true);
}
