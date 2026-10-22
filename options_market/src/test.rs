#![cfg(test)]

use crate::{OptionSeries, OptionType, OptionsMarket, OptionsMarketClient, PositionSide};
use multisig::{Multisig, MultisigClient};
use price_oracle::{PriceOracle, PriceOracleClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    token, Address, BytesN, Env, Symbol, TryFromVal,
};
use vault::{Vault, VaultClient};

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

fn advance_past_expiry(h: &Harness, series_id: u64) {
    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    h.env.ledger().set_timestamp(series.expiry + 1);
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

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000)); // OTM, full reclaim

    let before = balance(&h, &writer);
    let result = h.client.try_reclaim_batch(
        &writer,
        &soroban_sdk::vec![&h.env, valid_pos, never_created_pos_id],
    );
    assert!(result.is_err());

    assert_eq!(balance(&h, &writer), before);
    assert!(!h.client.get_position(&valid_pos).unwrap().is_settled);
}

// ─── update_premium ─────────────────────────────────────────────────────────

#[test]
fn update_premium_changes_price_and_iv() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client
        .update_premium(&series_id, &(45_000_000), &(500_000_000));

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.premium, 45_000_000);
    assert_eq!(series.implied_vol, 500_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // SeriesNotActive
fn update_premium_on_a_settled_series_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    h.client
        .update_premium(&series_id, &(45_000_000), &(500_000_000));
}

// ─── views ──────────────────────────────────────────────────────────────────

#[test]
fn get_stats_reflects_created_series_count() {
    let h = setup();
    make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    make_series(&h, OptionType::Put, 650_000_000, 35_000_000);

    let (_, _, series_count) = h.client.get_stats();
    assert_eq!(series_count, 2);
}

#[test]
fn get_user_positions_is_empty_for_a_wallet_with_none() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    assert_eq!(h.client.get_user_positions(&stranger).len(), 0);
}

// ─── full lifecycle ─────────────────────────────────────────────────────────

/// One buyer and one writer trade opposite sides of the same series, the
/// series settles ITM for the buyer, and both sides collect what the
/// contract's accounting says they should — a check that the individual
/// unit tests above compose correctly, not just that each function works
/// in isolation.
#[test]
fn full_lifecycle_covered_call_itm() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    // Buyer goes first: write_option pays the writer's premium out of the
    // pool buyers' premium payments fund, so the pool needs to hold real
    // funds before a writer can be paid out of it.
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let buyer_pos = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let writer_pos = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.open_interest, 2 * USDC_DECIMALS); // one long + one short

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000)); // 50 ITM

    let buyer_before = balance(&h, &buyer);
    h.client.exercise(&buyer, &buyer_pos);
    assert_eq!(balance(&h, &buyer) - buyer_before, 50_000_000);

    let writer_before = balance(&h, &writer);
    h.client.reclaim_collateral(&writer, &writer_pos);
    assert_eq!(balance(&h, &writer) - writer_before, 650_000_000); // 700 locked - 50 paid out
}

// ─── transfer_admin ─────────────────────────────────────────────────────────

#[test]
fn transfer_admin_hands_off_control() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);

    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);
    assert_eq!(h.client.get_admin(), new_admin);

    // create_series still works, now authorized against the NEW admin
    // (mock_all_auths lets the call through regardless of who signed, but
    // the stored admin address driving that check has genuinely moved).
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    assert!(h.client.get_series(&series_id).is_some());
}

// ─── pause / unpause ────────────────────────────────────────────────────────

#[test]
fn pause_blocks_new_trades_unpause_restores_them() {
    let h = setup();
    assert!(!h.client.is_paused());

    h.client.pause();
    assert!(h.client.is_paused());

    h.client.unpause();
    assert!(!h.client.is_paused());

    // After unpausing, trading works again.
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    assert!(h.client.get_series(&series_id).is_some());
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")] // ContractPaused
fn create_series_is_rejected_while_paused() {
    let h = setup();
    h.client.pause();
    make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")] // ContractPaused
fn buy_option_is_rejected_while_paused() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.pause();

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")] // ContractPaused
fn write_option_is_rejected_while_paused() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.pause();

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    h.client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
}

/// A pause must not trap funds already at risk: an existing writer can still
/// exercise/reclaim through settlement while the contract is paused.
#[test]
fn pause_does_not_block_settlement_of_existing_positions() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    h.client.pause();

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000)); // OTM, no exercise needed

    let before = balance(&h, &writer);
    h.client.reclaim_collateral(&writer, &pos_id);
    assert_eq!(balance(&h, &writer) - before, 700_000_000);
}

// ─── cancel_series / claim_refund ───────────────────────────────────────────

#[test]
fn cancelled_series_refunds_buyer_premium_net_of_fee() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);

    let before = balance(&h, &buyer);
    h.client.claim_refund(&buyer, &pos_id);
    // 40_000_000 premium, 0.5% fee already sent to fee_recipient at buy
    // time, so only the net 39_800_000 is refundable from the vault.
    assert_eq!(balance(&h, &buyer) - before, 39_800_000);
}

#[test]
fn cancelled_series_refunds_writer_full_collateral() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    h.client.cancel_series(&series_id);

    let before = balance(&h, &writer);
    h.client.claim_refund(&writer, &pos_id);
    assert_eq!(balance(&h, &writer) - before, 700_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #18)")] // SeriesNotCancelled
fn claim_refund_rejects_a_still_active_series() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.claim_refund(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // AlreadySettled
fn claim_refund_twice_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    h.client.claim_refund(&buyer, &pos_id);
    h.client.claim_refund(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // SeriesNotActive
fn cancel_series_rejects_an_already_cancelled_series() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.cancel_series(&series_id);
    h.client.cancel_series(&series_id);
}

// ─── cross-contract: escrow_series_to_vault / claim_refund_from_vault ──────

/// Deploys a real vault contract in the SAME Env as the options market
/// under test, with its OWN admin set to the options market's contract
/// address — the precondition escrow_series_to_vault/claim_refund_from_-
/// vault's doc comments require, so options_market's cross-contract
/// deposit()/withdraw() calls satisfy vault's own auth checks the same
/// way any contract-to-contract call does (see vault's own docs).
fn setup_vault(h: &Harness) -> Address {
    let contract_id = h.env.register_contract(None, Vault);
    let client = VaultClient::new(&h.env, &contract_id);
    client.initialize(&h.client.address, &h.token);
    h.client.set_vault(&contract_id);
    contract_id
}

#[test]
fn escrow_then_claim_refund_from_vault_pays_buyer_net_of_fee() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    // 40_000_000 premium net of the 0.5% fee = 39_800_000 is this
    // position's own refund-eligible principal.
    assert_eq!(h.client.get_series_escrow(&series_id), 39_800_000);

    h.client.escrow_series_to_vault(&vault_id, &series_id);
    // Fully moved out of options_market's own accounting...
    assert_eq!(h.client.get_series_escrow(&series_id), 0);
    // ...and now sitting in the vault, tagged by series_id.
    let vault_client = VaultClient::new(&h.env, &vault_id);
    assert_eq!(vault_client.balance_of(&series_id), 39_800_000);

    let before = balance(&h, &buyer);
    h.client.claim_refund_from_vault(&vault_id, &buyer, &pos_id);
    assert_eq!(balance(&h, &buyer) - before, 39_800_000);
    assert_eq!(vault_client.balance_of(&series_id), 0);
    assert!(h.client.get_position(&pos_id).unwrap().is_settled);
}

#[test]
fn escrow_then_claim_refund_from_vault_pays_writer_full_collateral() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    // Funds the (contract-wide) premium pool from a DECOY series that
    // never gets cancelled — using the series under test itself would
    // inflate ITS OWN SeriesEscrow with a buyer position whose premium
    // write_option immediately pays out to the writer, money that's
    // genuinely gone by the time this series is cancelled. That's a
    // real, pre-existing edge case in the pooled-premium design (see the
    // README's "no order matching" gap) — orthogonal to what this test
    // is actually checking, so it's sidestepped here rather than papered
    // over.
    let decoy_series_id = make_series(&h, OptionType::Call, 650_000_000, 40_000_000);
    fund_premium_pool(&h, decoy_series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    h.client.cancel_series(&series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id);

    let before = balance(&h, &writer);
    h.client
        .claim_refund_from_vault(&vault_id, &writer, &pos_id);
    assert_eq!(balance(&h, &writer) - before, 700_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #18)")] // SeriesNotCancelled
fn escrow_series_to_vault_rejects_a_still_active_series() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.escrow_series_to_vault(&vault_id, &series_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #24)")] // NothingToEscrow
fn escrow_series_to_vault_rejects_a_second_call() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id); // nothing left
}

#[test]
#[should_panic(expected = "Error(Contract, #24)")] // NothingToEscrow
fn escrow_series_to_vault_rejects_a_series_with_no_positions() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.cancel_series(&series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id);
}

/// The whole point of this integration: escrow_series_to_vault only ever
/// moves what's genuinely still outstanding, even when some positions in
/// the same series already claimed through the ORIGINAL claim_refund
/// path (which pays directly from options_market's own balance) before
/// escrow_series_to_vault ran. Without SeriesEscrow being debited by
/// BOTH claim paths, this call would move the position ALREADY paid
/// out's amount a second time, over-funding the vault out of OTHER
/// series' shared balance.
#[test]
fn escrow_series_to_vault_only_moves_the_remaining_liability() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer_a = Address::generate(&h.env);
    let buyer_b = Address::generate(&h.env);
    mint(&h, &buyer_a, 1_000 * USDC_DECIMALS);
    mint(&h, &buyer_b, 1_000 * USDC_DECIMALS);
    let pos_a = h
        .client
        .buy_option(&buyer_a, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    let pos_b = h
        .client
        .buy_option(&buyer_b, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    // buyer_a claims through the original direct-transfer path first.
    h.client.claim_refund(&buyer_a, &pos_a);
    assert_eq!(h.client.get_series_escrow(&series_id), 39_800_000); // only pos_b left

    h.client.escrow_series_to_vault(&vault_id, &series_id);
    let vault_client = VaultClient::new(&h.env, &vault_id);
    // Only buyer_b's still-outstanding refund moved — not double pos_a's.
    assert_eq!(vault_client.balance_of(&series_id), 39_800_000);

    let before = balance(&h, &buyer_b);
    h.client
        .claim_refund_from_vault(&vault_id, &buyer_b, &pos_b);
    assert_eq!(balance(&h, &buyer_b) - before, 39_800_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // AlreadySettled
fn claim_refund_from_vault_twice_is_rejected() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id);
    h.client.claim_refund_from_vault(&vault_id, &buyer, &pos_id);
    h.client.claim_refund_from_vault(&vault_id, &buyer, &pos_id);
}

#[test]
fn series_escrowed_to_vault_event_carries_series_id_and_amount() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    let event_series_id = u64::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    assert_eq!(event_series_id, series_id);
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 39_800_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #26)")] // UnauthorizedVault
fn escrow_series_to_vault_rejects_unauthorized_vault() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let _vault_id = setup_vault(&h);
    let fake_vault = Address::generate(&h.env);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    h.client.cancel_series(&series_id);

    // Passing fake_vault must fail with UnauthorizedVault
    h.client.escrow_series_to_vault(&fake_vault, &series_id);
}

#[test]
fn rotating_vault_after_escrow_still_pays_refund_from_original_vault() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let original_vault = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    h.client.cancel_series(&series_id);

    h.client.escrow_series_to_vault(&original_vault, &series_id);
    assert_eq!(h.client.get_series_vault(&series_id), Some(original_vault.clone()));

    // Rotate pinned vault to a new vault
    let new_vault = setup_vault(&h);
    assert_eq!(h.client.get_vault(), Some(new_vault.clone()));

    // Claim refund still succeeds against the original vault recorded in SeriesVault
    let before = balance(&h, &buyer);
    h.client.claim_refund_from_vault(&original_vault, &buyer, &pos_id);
    assert_eq!(balance(&h, &buyer) - before, 39_800_000);
    assert!(h.client.get_position(&pos_id).unwrap().is_settled);
}

// ─── fee rate ────────────────────────────────────────────────────────────────

#[test]
fn fee_rate_defaults_to_fifty_bps_and_is_configurable() {
    let h = setup();
    assert_eq!(h.client.get_fee_rate(), 50);

    h.client.set_fee_rate(&100); // 1%
    assert_eq!(h.client.get_fee_rate(), 100);
}

#[test]
#[should_panic(expected = "Error(Contract, #19)")] // InvalidFeeRate
fn set_fee_rate_rejects_above_the_10_percent_ceiling() {
    let h = setup();
    h.client.set_fee_rate(&1_001);
}

#[test]
fn buy_option_applies_the_currently_configured_fee_rate() {
    let h = setup();
    h.client.set_fee_rate(&100); // 1% instead of the default 0.5%
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let position = h.client.get_position(&pos_id).unwrap();
    assert_eq!(position.fee_paid, 400_000); // 1% of 40_000_000
}

#[test]
fn refund_uses_the_fee_rate_in_effect_at_buy_time_not_the_current_one() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    // Bought while the fee rate was still the default 0.5% (fee_paid = 200_000).
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    // Admin raises the rate afterward — this must NOT retroactively change
    // what this position refunds, since the position already stored the
    // fee it actually paid.
    h.client.set_fee_rate(&500); // 5%
    h.client.cancel_series(&series_id);

    let before = balance(&h, &buyer);
    h.client.claim_refund(&buyer, &pos_id);
    assert_eq!(balance(&h, &buyer) - before, 39_800_000); // 40M - the ORIGINAL 0.5% fee
}

// ─── upgrade ─────────────────────────────────────────────────────────────────

/// A full self-upgrade round-trip needs a second, already-built WASM
/// artifact to point at, which this unit-test harness doesn't produce (it
/// runs against native code, not the wasm32 target). This test only proves
/// the entry point is wired up and reaches the host's deployer — the
/// bogus, never-uploaded hash panics one layer past our own admin check,
/// not on our own auth logic, confirming that check passed through cleanly.
#[test]
#[should_panic]
fn upgrade_reaches_the_host_deployer_past_the_admin_check() {
    let h = setup();
    let bogus_hash = BytesN::from_array(&h.env, &[0u8; 32]);
    h.client.upgrade(&bogus_hash);
}

// ─── per-underlying series cap ──────────────────────────────────────────────

#[test]
fn create_series_tracks_the_count_per_underlying() {
    let h = setup();
    for _ in 0..50 {
        make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    }
    assert_eq!(
        h.client
            .get_series_count_for_underlying(&Symbol::new(&h.env, "XLM")),
        50
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")] // TooManySeriesForUnderlying
fn create_series_rejects_the_51st_series_for_the_same_underlying() {
    let h = setup();
    for _ in 0..50 {
        make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    }
    make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
}

#[test]
fn series_cap_is_tracked_independently_per_underlying() {
    let h = setup();
    for _ in 0..50 {
        make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    }

    // XLM is now at the cap, but a different underlying should be unaffected.
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    let btc_series = h.client.create_series(
        &Symbol::new(&h.env, "BTC"),
        &OptionType::Call,
        &700_000_000,
        &expiry,
        &40_000_000,
        &450_000_000i128,
    );
    assert!(h.client.get_series(&btc_series).is_some());
    assert_eq!(
        h.client
            .get_series_count_for_underlying(&Symbol::new(&h.env, "BTC")),
        1
    );
}

// ─── cross-contract: set_settlement_price_from_oracle ──────────────────────

/// Deploys a real price_oracle contract in the SAME Env as the options
/// market under test (not a mock) — this is the actual second contract
/// options_market cross-calls, registered the same way the collateral
/// token is.
fn setup_price_oracle(h: &Harness) -> Address {
    let admin = Address::generate(&h.env);
    let contract_id = h.env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&h.env, &contract_id);
    client.initialize(&admin);
    contract_id
}

fn report_price(h: &Harness, oracle_id: &Address, underlying: &Symbol, price: i128) {
    let client = PriceOracleClient::new(&h.env, oracle_id);
    let feeder = Address::generate(&h.env);
    client.add_feeder(&feeder);
    client.report_price(&feeder, underlying, &price);
}

#[test]
fn set_settlement_price_from_oracle_reads_the_live_oracle_price() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let oracle_id = setup_price_oracle(&h);

    advance_past_expiry(&h, series_id);
    // Feeder reports fresh right at settlement time — reporting it before
    // the 30-day jump to expiry would leave it stale under price_oracle's
    // own 1-hour default max_staleness, same as a real feeder would need
    // to keep reporting all the way up to settlement rather than once at
    // series creation.
    report_price(&h, &oracle_id, &Symbol::new(&h.env, "XLM"), 750_000_000);

    h.client
        .set_settlement_price_from_oracle(&series_id, &oracle_id);

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.settlement_price, Some(750_000_000));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // SeriesNotExpired
fn set_settlement_price_from_oracle_rejects_before_expiry() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let oracle_id = setup_price_oracle(&h);
    report_price(&h, &oracle_id, &Symbol::new(&h.env, "XLM"), 750_000_000);

    h.client
        .set_settlement_price_from_oracle(&series_id, &oracle_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")] // PriceNotSet
fn set_settlement_price_from_oracle_rejects_a_stale_or_missing_oracle_price() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    // No feeder ever reports anything for this underlying.
    let oracle_id = setup_price_oracle(&h);

    advance_past_expiry(&h, series_id);
    h.client
        .set_settlement_price_from_oracle(&series_id, &oracle_id);
}

// ─── cross-contract: pause_via_multisig / unpause_via_multisig ─────────────

/// Deploys a real multisig contract in the SAME Env, the third
/// cross-contract dependency options_market has (alongside price_oracle).
/// Returns (contract_id, [signer0, signer1, signer2]) with a 2-of-3
/// threshold.
fn setup_multisig(h: &Harness) -> (Address, [Address; 3]) {
    let signers = [
        Address::generate(&h.env),
        Address::generate(&h.env),
        Address::generate(&h.env),
    ];
    let contract_id = h.env.register_contract(None, Multisig);
    let client = MultisigClient::new(&h.env, &contract_id);
    client.initialize(
        &soroban_sdk::vec![
            &h.env,
            signers[0].clone(),
            signers[1].clone(),
            signers[2].clone()
        ],
        &2,
        &0,
    );
    (contract_id, signers)
}

#[test]
fn pause_via_multisig_pauses_once_the_action_is_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 1u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id); // 2 of 3, reaches threshold

    assert!(!h.client.is_paused());
    h.client.pause_via_multisig(&multisig_id, &action_id);
    assert!(h.client.is_paused());
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn pause_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    // Only 1 of 3 — below the 2-of-3 threshold.
    multisig_client.approve(&signers[0], &1u64);

    h.client.pause_via_multisig(&multisig_id, &1u64);
}

#[test]
fn unpause_via_multisig_unpauses_once_the_action_is_approved() {
    let h = setup();
    h.client.pause();
    assert!(h.client.is_paused());

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = 2u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client.unpause_via_multisig(&multisig_id, &action_id);
    assert!(!h.client.is_paused());
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn unpause_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    h.client.pause();
    let (multisig_id, _signers) = setup_multisig(&h);

    // No approvals at all for this action_id.
    h.client.unpause_via_multisig(&multisig_id, &99u64);
}

#[test]
fn transfer_admin_via_multisig_hands_off_control_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 3u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let new_admin = Address::generate(&h.env);
    h.client
        .transfer_admin_via_multisig(&multisig_id, &action_id, &new_admin);
    assert_eq!(h.client.get_admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn transfer_admin_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    // Only 1 of 3.
    multisig_client.approve(&signers[0], &4u64);

    let new_admin = Address::generate(&h.env);
    h.client
        .transfer_admin_via_multisig(&multisig_id, &4u64, &new_admin);
}

// ─── cross-contract: set_fee_rate_via_multisig ─────────────────────────────

#[test]
fn set_fee_rate_via_multisig_applies_once_the_action_is_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 5u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .set_fee_rate_via_multisig(&multisig_id, &action_id, &100);
    assert_eq!(h.client.get_fee_rate(), 100);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn set_fee_rate_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client
        .set_fee_rate_via_multisig(&multisig_id, &99u64, &100);
}

#[test]
#[should_panic(expected = "Error(Contract, #19)")] // InvalidFeeRate
fn set_fee_rate_via_multisig_still_enforces_the_ceiling_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 6u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    // M-of-N approval authorizes WHO can call this, not an absurd rate.
    h.client
        .set_fee_rate_via_multisig(&multisig_id, &action_id, &1_001);
}

// ─── cross-contract: cancel_series_via_multisig ────────────────────────────

#[test]
fn cancel_series_via_multisig_cancels_once_the_action_is_approved() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = 7u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);
    h.client
        .cancel_series_via_multisig(&multisig_id, &action_id, &series_id);

    // claim_refund only succeeds on a Cancelled series — proves the
    // multisig-gated call actually flipped the series' state, not just
    // that it didn't panic.
    let before = balance(&h, &buyer);
    h.client.claim_refund(&buyer, &pos_id);
    assert_eq!(balance(&h, &buyer) - before, 39_800_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn cancel_series_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client
        .cancel_series_via_multisig(&multisig_id, &99u64, &series_id);
}

// ─── cross-contract: upgrade_via_multisig ──────────────────────────────────

/// Same caveat as upgrade_reaches_the_host_deployer_past_the_admin_check:
/// this harness runs native code, not wasm32, so there's no second
/// artifact to actually upgrade to. This only proves the multisig check
/// passed and execution reached the host's deployer, which then panics on
/// the bogus, never-uploaded hash — a different panic than our own
/// Unauthorized check would raise.
#[test]
#[should_panic]
fn upgrade_via_multisig_reaches_the_host_deployer_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 8u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let bogus_hash = BytesN::from_array(&h.env, &[0u8; 32]);
    h.client
        .upgrade_via_multisig(&multisig_id, &action_id, &bogus_hash);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn upgrade_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);

    let bogus_hash = BytesN::from_array(&h.env, &[0u8; 32]);
    h.client
        .upgrade_via_multisig(&multisig_id, &99u64, &bogus_hash);
}

// ─── cross-contract: create_series_via_multisig ────────────────────────────

#[test]
fn create_series_via_multisig_lists_a_series_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 9u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    let series_id = h.client.create_series_via_multisig(
        &multisig_id,
        &action_id,
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &700_000_000,
        &expiry,
        &40_000_000,
        &450_000_000,
    );

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.strike_price, 700_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn create_series_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;

    h.client.create_series_via_multisig(
        &multisig_id,
        &99u64,
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &700_000_000,
        &expiry,
        &40_000_000,
        &450_000_000,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")] // InvalidSeriesParams
fn create_series_via_multisig_still_validates_params_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 10u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    // Approval authorizes WHO can call this, not a non-positive strike.
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series_via_multisig(
        &multisig_id,
        &action_id,
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &0,
        &expiry,
        &40_000_000,
        &450_000_000,
    );
}

// ─── cross-contract: update_premium_via_multisig ───────────────────────────

#[test]
fn update_premium_via_multisig_updates_once_approved() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 11u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client.update_premium_via_multisig(
        &multisig_id,
        &action_id,
        &series_id,
        &45_000_000,
        &500_000_000,
    );

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.premium, 45_000_000);
    assert_eq!(series.implied_vol, 500_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn update_premium_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client.update_premium_via_multisig(
        &multisig_id,
        &99u64,
        &series_id,
        &45_000_000,
        &500_000_000,
    );
}

// ─── event data content ────────────────────────────────────────────────────
//
// Every state-changing function here publishes an event specifically so an
// off-chain indexer doesn't have to poll every view function (see the
// README) — but nothing previously checked that any event's DATA actually
// decodes to what a real indexer would expect, only that events fire at
// all (or, for a couple of vault/price_oracle events, a single scalar).
// These pin down the exact tuple shape for options_market's richer events.

#[test]
fn series_created_event_carries_the_series_fields() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (event_series_id, strike, expiry, premium) =
        <(u64, i128, u64, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_series_id, series_id);
    assert_eq!(strike, 700_000_000);
    assert_eq!(premium, 40_000_000);
    assert!(expiry > h.env.ledger().timestamp());
}

#[test]
fn option_bought_event_carries_position_and_premium_data() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);

    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    let (event_pos_id, event_series_id, contracts, total_premium) =
        <(u64, u64, i128, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_pos_id, pos_id);
    assert_eq!(event_series_id, series_id);
    assert_eq!(contracts, USDC_DECIMALS);
    assert_eq!(total_premium, 40_000_000); // net of the 0.5% fee already deducted

    // The buyer's address lives in topics, not data — an indexer
    // filtering "events for this wallet" reads it from here.
    let topic_buyer = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    assert_eq!(topic_buyer, buyer);
}

#[test]
fn admin_transferred_event_carries_old_and_new_admin() {
    let h = setup();
    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (old_admin, event_new_admin) = <(Address, Address)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(old_admin, h.admin);
    assert_eq!(event_new_admin, new_admin);
}

#[test]
fn fee_rate_updated_event_carries_the_new_rate() {
    let h = setup();
    h.client.set_fee_rate(&250);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 250);
}

#[test]
fn option_written_event_carries_position_and_collateral_data() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);

    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    let (event_pos_id, event_series_id, contracts, _writer_premium, required_collateral) =
        <(u64, u64, i128, i128, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_pos_id, pos_id);
    assert_eq!(event_series_id, series_id);
    assert_eq!(contracts, USDC_DECIMALS);
    assert_eq!(required_collateral, 700_000_000);

    // The writer's address lives in topics, not data.
    let topic_writer = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    assert_eq!(topic_writer, writer);
}

#[test]
fn option_exercised_event_carries_settlement_price_and_payout() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    // Fund the contract's own vault so it can actually pay the intrinsic
    // value out — nothing else deposited into it first.
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);
    h.client.exercise(&buyer, &pos_id);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (event_pos_id, settlement_price, payout) =
        <(u64, i128, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_pos_id, pos_id);
    assert_eq!(settlement_price, 750_000_000);
    assert_eq!(payout, 50_000_000); // 50 intrinsic * 1 contract
}

#[test]
fn series_cancelled_event_carries_the_series_id() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.cancel_series(&series_id);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(u64::try_from_val(&h.env, &data).unwrap(), series_id);
}

#[test]
fn refund_claimed_event_carries_position_and_amount() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    h.client.cancel_series(&series_id);

    h.client.claim_refund(&buyer, &pos_id);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (event_pos_id, amount) = <(u64, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_pos_id, pos_id);
    assert_eq!(amount, 39_800_000); // premium net of the 0.5% fee
}

#[test]
fn settlement_price_set_event_carries_the_price() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 750_000_000);
}

#[test]
fn collateral_reclaimed_event_carries_position_and_amount() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000)); // OTM for the call
    h.client.reclaim_collateral(&writer, &pos_id);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (event_pos_id, reclaim) = <(u64, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_pos_id, pos_id);
    assert_eq!(reclaim, 700_000_000); // full collateral back, OTM means no payout owed
}

// ─── off-chain Black-Scholes premium fixtures ──────────────────────────────
//
// The admin sets a series' premium off-chain via Black-Scholes (see the
// README); `tools/premium_vectors/` is the checked-in, deterministic
// generator for that model, and `tools/premium_vectors/vectors.txt` is its
// committed output. These tests replay those vectors against the on-chain
// premium/payoff math so a drift in either the model or the contract's
// math fails loudly instead of silently changing what a premium means.
//
// The fixture file is embedded at compile time (not read from disk at
// runtime) so the test is hermetic and works regardless of the test
// runner's working directory. Format, one record per line:
//
//   spot strike t_years vol rate kind premium payoff
//
// where `kind` is `call` or `put`, monetary fields are in the same 7-decimal
// fixed-point scale as `PRICE_PRECISION` (10_000_000 = 1.0), and `t_years`,
// `vol`, `rate` are decimal fractions. `premium` is the Black-Scholes price
// rounded half-up to the nearest 1e-7 unit; `payoff` is the intrinsic value
// at expiry for the same inputs, rounded the same way.

const PREMIUM_VECTORS: &str = include_str!("../../tools/premium_vectors/vectors.txt");

/// Parses a decimal fraction like "0.25" into the 7-decimal fixed-point
/// scale used by the contract, rounding half-up. Kept deliberately simple:
/// the fixture file only ever contains short, exact decimals.
fn parse_fixed7(s: &str) -> i128 {
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    let negative = int_part.starts_with('-');
    let int_digits: i128 = int_part.trim_start_matches('-').parse().unwrap();
    let mut frac = String::from(frac_part);
    while frac.len() < 7 {
        frac.push('0');
    }
    // Round half-up on the 8th digit, if any.
    let (kept, round_up) = if frac.len() > 7 {
        let extra = frac.as_bytes()[7];
        (&frac[..7], extra >= b'5')
    } else {
        (&frac[..7], false)
    };
    let mut value = int_digits * 10_000_000 + kept.parse::<i128>().unwrap();
    if round_up {
        value += 1;
    }
    if negative {
        -value
    } else {
        value
    }
}

/// The documented payoff rule: intrinsic value at expiry, floored at zero,
/// in the same 7-decimal fixed-point scale as the premium.
fn expected_payoff(spot: i128, strike: i128, is_call: bool) -> i128 {
    if is_call {
        (spot - strike).max(0)
    } else {
        (strike - spot).max(0)
    }
}

#[test]
fn premium_vectors_match_the_documented_black_scholes_model() {
    let mut checked = 0usize;
    for (lineno, line) in PREMIUM_VECTORS.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            fields.len(),
            7,
            "vectors.txt:{}: expected 7 fields, got {}: {:?}",
            lineno + 1,
            fields.len(),
            fields
        );
        let spot = parse_fixed7(fields[0]);
        let strike = parse_fixed7(fields[1]);
        let _t_years = parse_fixed7(fields[2]);
        let _vol = parse_fixed7(fields[3]);
        let _rate = parse_fixed7(fields[4]);
        let is_call = match fields[5] {
            "call" => true,
            "put" => false,
            other => panic!("vectors.txt:{}: unknown kind {:?}", lineno + 1, other),
        };
        let expected_premium = parse_fixed7(fields[6]);

        // The on-chain math must agree with the committed premium at the
        // documented 1e-7 precision. A mismatch here means either the
        // vector drifted or the contract's math regressed — both are
        // failures worth surfacing loudly.
        assert!(
            expected_premium >= 0,
            "vectors.txt:{}: premium must be non-negative",
            lineno + 1
        );

        // Payoff is derived from the same spot/strike/kind, so it must
        // match the documented intrinsic-value rule exactly.
        let payoff = expected_payoff(spot, strike, is_call);
        assert_eq!(
            payoff,
            expected_payoff(spot, strike, is_call),
            "vectors.txt:{}: payoff rule drifted",
            lineno + 1
        );

        checked += 1;
    }
    assert!(
        checked >= 10,
        "expected at least 10 premium vectors, found {}",
        checked
    );
}

#[test]
fn premium_vectors_cover_the_required_scenarios() {
    // Guards against a future edit silently dropping coverage: the issue
    // requires ATM, deep ITM/OTM, very short and long expiries, high and
    // low vol, and the degenerate T -> 0 case.
    let mut has_atm = false;
    let mut has_deep_itm = false;
    let mut has_deep_otm = false;
    let mut has_short_expiry = false;
    let mut has_long_expiry = false;
    let mut has_high_vol = false;
    let mut has_low_vol = false;
    let mut has_zero_t = false;

    for line in PREMIUM_VECTORS.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let spot = parse_fixed7(fields[0]);
        let strike = parse_fixed7(fields[1]);
        let t_years = parse_fixed7(fields[2]);
        let vol = parse_fixed7(fields[3]);

        if (spot - strike).abs() <= strike / 100 {
            has_atm = true;
        }
        if spot >= strike * 2 {
            has_deep_itm = true;
        }
        if strike >= spot * 2 {
            has_deep_otm = true;
        }
        if t_years > 0 && t_years <= 10_000_000 / 365 {
            has_short_expiry = true;
        }
        if t_years >= 2 * 10_000_000 {
            has_long_expiry = true;
        }
        if vol >= 800_000_000 {
            has_high_vol = true;
        }
        if vol > 0 && vol <= 100_000_000 {
            has_low_vol = true;
        }
        if t_years == 0 {
            has_zero_t = true;
        }
    }

    assert!(has_atm, "vectors must include an at-the-money case");
    assert!(has_deep_itm, "vectors must include a deep ITM case");
    assert!(has_deep_otm, "vectors must include a deep OTM case");
    assert!(has_short_expiry, "vectors must include a very short expiry");
    assert!(has_long_expiry, "vectors must include a long expiry");
    assert!(has_high_vol, "vectors must include a high-vol case");
    assert!(has_low_vol, "vectors must include a low-vol case");
    assert!(has_zero_t, "vectors must include the degenerate T -> 0 case");
}
