#![cfg(test)]

use crate::{OptionSeries, OptionType, OptionsMarket, OptionsMarketClient, PositionSide};
use multisig::{Multisig, MultisigClient};
use price_oracle::{PriceOracle, PriceOracleClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    token, Address, BytesN, Env, Symbol, TryFromVal,
};
use vault::{Vault, VaultClient};
use zenith_common::testutils::{advance_timestamp, set_timestamp};

const USDC_DECIMALS: i128 = 10_000_000; // matches PRICE_PRECISION

struct Harness<'a> {
    env: Env,
    client: OptionsMarketClient<'a>,
    token: Address,
    admin: Address,
    oracle: Address,
    fee_recipient: Address,
}

/// Registers a Stellar Asset Contract as the collateral token (a real
/// deployable token, not a hand-rolled mock) and the options market
/// contract itself, then initializes the market. `mock_all_auths()`
/// means every `require_auth()` call in the contract succeeds without
/// needing real signatures — appropriate for unit tests that are
/// exercising the contract's own logic, not its auth plumbing.
fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let fee_recipient = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_address = token_contract.address();

    let contract_id = env.register_contract(None, OptionsMarket);
    let client = OptionsMarketClient::new(&env, &contract_id);
    client.initialize(&admin, &oracle, &token_address, &fee_recipient);

    Harness {
        env,
        client,
        token: token_address,
        admin,
        oracle,
        fee_recipient,
    }
}

/// Mints `amount` of the test collateral token to `to` via the Stellar
/// Asset Contract's admin-only mint entry point.
fn mint(h: &Harness, to: &Address, amount: i128) {
    token::StellarAssetClient::new(&h.env, &h.token).mint(to, &amount);
}

fn balance(h: &Harness, of: &Address) -> i128 {
    token::Client::new(&h.env, &h.token).balance(of)
}

fn make_series(h: &Harness, option_type: OptionType, strike: i128, premium: i128) -> u64 {
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series(
        &Symbol::new(&h.env, "XLM"),
        &option_type,
        &strike,
        &expiry,
        &premium,
        &(450_000_000i128), // 45% implied vol
    )
}

/// write_option pays a writer's premium out of the pool that buyers' own
/// premium payments fund (see the InsufficientPremiumPool gate in
/// write_option) — so tests that only care about the writer side still
/// need a throwaway buyer to have funded that pool first. Buying the same
/// `contracts` count in the same series contributes exactly the
/// fee-adjusted amount write_option will need to pay out.
fn fund_premium_pool(h: &Harness, series_id: u64, contracts: i128) {
    let filler_buyer = Address::generate(&h.env);
    mint(h, &filler_buyer, 1_000 * USDC_DECIMALS);
    h.client.buy_option(
        &filler_buyer,
        &series_id,
        &contracts,
        &(1_000 * USDC_DECIMALS),
    );
}

#[test]
fn initialize_sets_admin_and_zeroed_counters() {
    let h = setup();
    let (premiums, oi, series_count) = h.client.get_stats();
    assert_eq!(premiums, 0);
    assert_eq!(oi, 0);
    assert_eq!(series_count, 0);
    // Just confirms admin/oracle/fee_recipient/token were all accepted by
    // initialize without panicking — no direct getter for them exists.
    let _ = (&h.admin, &h.oracle, &h.fee_recipient);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn initialize_twice_panics() {
    let h = setup();
    h.client
        .initialize(&h.admin, &h.oracle, &h.token, &h.fee_recipient);
}

#[test]
fn create_series_stores_correct_fields() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let series: OptionSeries = h.client.get_series(&series_id).unwrap();

    assert_eq!(series.series_id, series_id);
    assert_eq!(series.strike_price, 700_000_000);
    assert_eq!(series.premium, 40_000_000);
    assert_eq!(series.open_interest, 0);
    assert!(series.option_type == OptionType::Call);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")] // ExpiryTooSoon
fn create_series_rejects_expiry_less_than_an_hour_out() {
    let h = setup();
    let too_soon = h.env.ledger().timestamp() + 1800; // 30 minutes
    h.client.create_series(
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &700_000_000,
        &too_soon,
        &40_000_000,
        &450_000_000,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")] // InvalidSeriesParams
fn create_series_rejects_a_non_positive_strike() {
    let h = setup();
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series(
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &0,
        &expiry,
        &40_000_000,
        &450_000_000,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")] // InvalidSeriesParams
fn create_series_rejects_a_negative_premium() {
    let h = setup();
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series(
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &700_000_000,
        &expiry,
        &-1,
        &450_000_000,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")] // InvalidSeriesParams
fn create_series_rejects_a_negative_implied_vol() {
    let h = setup();
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series(
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &700_000_000,
        &expiry,
        &40_000_000,
        &-1,
    );
}

#[test]
#[should_panic] // no auth was mocked at all — require_auth() has nothing to accept
fn initialize_without_any_authorization_panics() {
    // Deliberately skips mock_all_auths(): every other test in this file
    // uses it (soroban's mock-auth mode applies for the env's whole
    // lifetime once armed, so there's no way to "unmock" partway through
    // a test to check a *later* call specifically) — this test exists
    // just to confirm require_auth() is actually load-bearing on
    // initialize(), not a no-op, by never arming it in the first place.
    let env = Env::default();
    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let fee_recipient = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register_contract(None, OptionsMarket);
    let client = OptionsMarketClient::new(&env, &contract_id);
    client.initialize(&admin, &oracle, &token, &fee_recipient);
}

// ─── buy_option ─────────────────────────────────────────────────────────────

#[test]
fn buy_option_transfers_premium_and_opens_a_long_position() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);

    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let position = h.client.get_position(&pos_id).unwrap();
    assert!(position.side == PositionSide::Long);
    assert_eq!(position.contracts, USDC_DECIMALS);
    assert_eq!(position.collateral_locked, 0);
    // Buyer paid the full premium (40); the 0.5% protocol fee comes out of
    // what the vault later pays the writer, not an extra charge to the buyer.
    assert_eq!(balance(&h, &buyer), 1_000 * USDC_DECIMALS - 40_000_000);

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.open_interest, USDC_DECIMALS);

    let positions = h.client.get_user_positions(&buyer);
    assert_eq!(positions.len(), 1);
    assert_eq!(positions.get(0).unwrap(), pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")] // ZeroContracts
fn buy_option_rejects_zero_contracts() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client
        .buy_option(&buyer, &series_id, &0, &(50 * USDC_DECIMALS));
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // InsufficientPremium
fn buy_option_enforces_slippage_protection() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    // Series premium is 40_000_000 for 1 contract; offering a max_premium
    // below that must be rejected rather than silently charging more than
    // the caller agreed to.
    h.client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &30_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // SeriesNotFound
fn buy_option_rejects_unknown_series() {
    let h = setup();
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client
        .buy_option(&buyer, &999, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
}

// ─── write_option ───────────────────────────────────────────────────────────

#[test]
fn write_covered_call_locks_collateral_equal_to_notional() {
    let h = setup();
    // No underlying price has been set yet, so write_option falls back to
    // the series' own strike as the notional basis — this pins down that
    // documented fallback rather than assuming an oracle price exists.
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    let required = 700_000_000; // 1 contract * strike (fallback price)
    mint(&h, &writer, required);

    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &required);
    let position = h.client.get_position(&pos_id).unwrap();

    assert!(position.side == PositionSide::Short);
    assert_eq!(position.collateral_locked, required);
    // Writer received premium (minus the 0.5% protocol fee) on top of
    // having already spent `required` on collateral.
    let fee = 40_000_000 * 5 / 1000;
    assert_eq!(balance(&h, &writer), 40_000_000 - fee);
}

#[test]
fn write_cash_secured_put_requires_110_percent_of_strike() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Put, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    let required = 700_000_000 * 11 / 10; // strike * contracts * 110%
    mint(&h, &writer, required);

    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &required);
    let position = h.client.get_position(&pos_id).unwrap();
    assert_eq!(position.collateral_locked, required);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // InsufficientCollateral
fn write_option_rejects_undercollateralized_offer() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Put, 700_000_000, 40_000_000);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    // Offers exactly 100% of strike for a put, which needs 110%.
    h.client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #21)")] // InsufficientPremiumPool
fn write_option_rejects_a_write_with_no_buyer_premium_to_draw_from() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    // No buyer has ever bought into this series, so the premium pool is
    // empty — write_option must not pay the writer out of its own
    // just-deposited collateral, which isn't a premium anyone paid.
    h.client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
}

#[test]
fn write_option_succeeds_once_the_pool_partially_covers_it() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    assert_eq!(h.client.get_premium_pool(), 0);

    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    assert_eq!(h.client.get_premium_pool(), 39_800_000); // 40M premium net of the 0.5% fee

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    h.client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    // The writer's premium exactly drained the pool the lone buyer funded.
    assert_eq!(h.client.get_premium_pool(), 0);
}

// ─── settlement + exercise ──────────────────────────────────────────────────

/// Advances the ledger past `series_id`'s expiry using the shared
/// time-travel helper, so the timestamp jump goes through the same
/// monotonicity-checked path every other time-dependent crate uses.
fn advance_past_expiry(h: &Harness, series_id: u64) {
    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    set_timestamp(&h.env, series.expiry + 1);
}

#[test]
fn exercise_itm_call_pays_out_the_intrinsic_value() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    // Settlement price above strike -> call finishes in the money.
    h.client.set_settlement_price(&series_id, &(750_000_000));

    // Fund the contract's own vault so it can actually pay the intrinsic
    // value out — in this test nothing else deposited into it first.
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    let before = balance(&h, &buyer);
    h.client.exercise(&buyer, &pos_id);
    // Intrinsic = (750 - 700) * 1 contract = 50.
    assert_eq!(balance(&h, &buyer) - before, 50_000_000);

    let position = h.client.get_position(&pos_id).unwrap();
    assert!(position.is_exercised);
}

#[test]
fn exercise_itm_put_pays_out_the_intrinsic_value() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Put, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    // Settlement price below strike -> put finishes in the money.
    h.client.set_settlement_price(&series_id, &(650_000_000));
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    let before = balance(&h, &buyer);
    h.client.exercise(&buyer, &pos_id);
    // Intrinsic = (700 - 650) * 1 contract = 50.
    assert_eq!(balance(&h, &buyer) - before, 50_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")] // NotInTheMoney
fn exercise_otm_call_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    // Settlement below strike -> call is worthless.
    h.client.set_settlement_price(&series_id, &(650_000_000));
    h.client.exercise(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // SeriesNotExpired
fn exercise_before_expiry_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    h.client.exercise(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")] // WrongSide
fn exercise_rejects_a_short_position() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    h.client.exercise(&writer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // AlreadyExercised
fn exercise_twice_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    h.client.exercise(&buyer, &pos_id);
    h.client.exercise(&buyer, &pos_id);
}

// ─── exercise_batch ─────────────────────────────────────────────────────────

#[test]
fn exercise_batch_pays_out_every_position_in_the_list() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_a = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    let pos_b = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    let before = balance(&h, &buyer);
    let total = h
        .client
        .exercise_batch(&buyer, &soroban_sdk::vec![&h.env, pos_a, pos_b]);
    // Intrinsic = 50 per position, two positions.
    assert_eq!(total, 100_000_000);
    assert_eq!(balance(&h, &buyer) - before, 100_000_000);

    assert!(h.client.get_position(&pos_a).unwrap().is_exercised);
    assert!(h.client.get_position(&pos_b).unwrap().is_exercised);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")] // InvalidBatchSize
fn exercise_batch_rejects_an_empty_list() {
    let h = setup();
    let buyer = Address::generate(&h.env);
    h.client.exercise_batch(&buyer, &soroban_sdk::vec![&h.env]);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")] // InvalidBatchSize
fn exercise_batch_rejects_more_than_the_max_batch_size() {
    let h = setup();
    let buyer = Address::generate(&h.env);
    let mut ids = soroban_sdk::vec![&h.env];
    for i in 0..26u64 {
        ids.push_back(i);
    }
    h.client.exercise_batch(&buyer, &ids);
}

#[test]
fn exercise_batch_is_all_or_nothing() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let itm_pos = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    let never_created_pos_id = 9_999u64;

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    let before = balance(&h, &buyer);
    let result = h.client.try_exercise_batch(
        &buyer,
        &soroban_sdk::vec![&h.env, itm_pos, never_created_pos_id],
    );
    assert!(result.is_err());

    // The whole call aborted — the otherwise-valid ITM position in the
    // same batch was never paid out or marked exercised.
    assert_eq!(balance(&h, &buyer), before);
    assert!(!h.client.get_position(&itm_pos).unwrap().is_exercised);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn exercise_batch_rejects_a_position_owned_by_someone_else() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    let stranger = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    h.client
        .exercise_batch(&stranger, &soroban_sdk::vec![&h.env, pos_id]);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // SeriesNotExpired
fn set_settlement_price_before_expiry_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.set_settlement_price(&series_id, &(750_000_000));
}

// ─── reclaim_collateral ─────────────────────────────────────────────────────

#[test]
fn reclaim_collateral_returns_locked_minus_max_loss() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000)); // ITM by 50

    let before = balance(&h, &writer);
    h.client.reclaim_collateral(&writer, &pos_id);
    // 700 locked - 50 max loss = 650 reclaimed.
    assert_eq!(balance(&h, &writer) - before, 650_000_000);

    let position = h.client.get_position(&pos_id).unwrap();
    assert!(position.is_settled);
}

#[test]
fn reclaim_collateral_returns_everything_when_otm() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    // Funds the premium pool so write_option below has real buyer premium
    // to pay the writer from, instead of dipping into the writer's own
    // just-deposited collateral (see write_option's InsufficientPremiumPool
    // gate) — the fix for the vault-liquidity gap this test used to work
    // around with a direct top-up mint.
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000)); // OTM

    let before = balance(&h, &writer);
    h.client.reclaim_collateral(&writer, &pos_id);
    assert_eq!(balance(&h, &writer) - before, 700_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")] // WrongSide
fn reclaim_collateral_rejects_a_long_position() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    h.client.reclaim_collateral(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // AlreadySettled
fn reclaim_collateral_twice_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    // Funds the premium pool for the same reason noted on
    // reclaim_collateral_returns_everything_when_otm — this used to need a
    // direct top-up mint to make the FIRST reclaim below succeed for real
    // (otherwise it fails on the token contract's own insufficient-balance
    // error, which happens to also render as "Error(Contract, #10)", so
    // the should_panic would pass for the wrong reason without ever
    // reaching the second call).
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000));
    h.client.reclaim_collateral(&writer, &pos_id);
    h.client.reclaim_collateral(&writer, &pos_id);
}

// ─── reclaim_batch ──────────────────────────────────────────────────────────

#[test]
fn reclaim_batch_pays_out_every_position_in_the_list() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, 2 * USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 1_400_000_000);
    let pos_a = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
    let pos_b = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000)); // ITM by 50 each

    let before = balance(&h, &writer);
    let total = h
        .client
        .reclaim_batch(&writer, &soroban_sdk::vec![&h.env, pos_a, pos_b]);
    // (700 locked - 50 max loss) * 2 positions = 1300.
    assert_eq!(total, 1_300_000_000);
    assert_eq!(balance(&h, &writer) - before, 1_300_000_000);

    assert!(h.client.get_position(&pos_a).unwrap().is_settled);
    assert!(h.client.get_position(&pos_b).unwrap().is_settled);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")] // InvalidBatchSize
fn reclaim_batch_rejects_an_empty_list() {
    let h = setup();
    let writer = Address::generate(&h.env);
    h.client.reclaim_batch(&writer, &soroban_sdk::vec![&h.env]);
}

#[test]
fn reclaim_batch_is_all_or_nothing() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let valid_pos = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
    let never_created_pos_id = 9_999u64;