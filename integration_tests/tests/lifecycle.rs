//! End-to-end option lifecycle (issue #178): price_oracle, vault and
//! options_market deployed into one Env and driven only through their
//! public entrypoints — write with collateral escrowed in the vault, buy,
//! feeders report, settle through the oracle, both sides claim — plus the
//! failure paths that live at the seams between contracts.
//!
//! These run offline under `cargo test` with the default resource budget
//! (no `budget().reset_unlimited()`), and fail to compile if any public
//! entrypoint they use is removed or renamed.

mod fixtures;

use fixtures::{deploy, Protocol, DAY, UNIT};
use options_market::{OptionType, PositionSide, SeriesState};
use soroban_sdk::{testutils::Address as _, Address, IntoVal, TryFromVal};

const STRIKE: i128 = 70 * UNIT;
const PREMIUM: i128 = 4 * UNIT;

/// Buyer opens `contracts` longs; returns (buyer, position_id).
fn buy(p: &Protocol, series_id: u64, contracts: i128) -> (Address, u64) {
    let buyer = p.user(1_000 * UNIT);
    let pos = p
        .market
        .buy_option(&buyer, &series_id, &contracts, &(1_000 * UNIT));
    (buyer, pos)
}

/// Writer locks the minimum collateral for `contracts`; returns
/// (writer, position_id, collateral).
fn write(p: &Protocol, series_id: u64, contracts: i128) -> (Address, u64, i128) {
    let writer = p.user(10_000 * UNIT);
    let before = p.balance(&writer);
    let pos = p
        .market
        .write_option(&writer, &series_id, &contracts, &(10_000 * UNIT), &0);
    let collateral = p.market.get_position(&pos).unwrap().collateral_locked;
    // The writer paid collateral and received the premium net of fee.
    let premium = p.market.get_position(&pos).unwrap().premium_paid;
    assert_eq!(before - p.balance(&writer), collateral - premium);
    (writer, pos, collateral)
}

#[test]
fn happy_path_itm_call_from_write_to_both_claims() {
    let p = deploy();
    let series_id = p.list_series(OptionType::Call, STRIKE, PREMIUM);
    assert!(p.market.is_vault_backed(&series_id));

    // Buyer pays premium; fee goes to the fee recipient.
    let (buyer, long) = buy(&p, series_id, 2 * UNIT);
    let fee = p.market.get_position(&long).unwrap().fee_paid;
    assert_eq!(p.balance(&p.fee_recipient), fee);
    assert!(p
        .event_names(&p.market.address)
        .contains(&"option_bought".into()));

    // Writer's collateral lands in the vault, under this series' tag.
    let (writer, short, collateral) = write(&p, series_id, 2 * UNIT);
    assert_eq!(collateral, 140 * UNIT); // covered call: 2 × strike notional
    assert_eq!(p.vault.balance_of(&series_id), collateral);
    assert_eq!(p.vault.get_total_escrowed(), collateral);
    assert!(p
        .event_names(&p.vault.address)
        .contains(&"deposited".into()));

    // Expiry: feeders report, one of them wildly off; anyone records a
    // round, then anyone settles the series through the oracle.
    p.warp_to(p.expiry(series_id));
    p.report(&[75 * UNIT, 75 * UNIT, 900 * UNIT]);
    let round = p.oracle.aggregate(&p.xlm());
    assert_eq!(round.price, 75 * UNIT);
    assert_eq!((round.reports_used, round.reports_discarded), (2, 1));
    p.market
        .set_settlement_price_from_oracle(&series_id, &p.oracle.address);
    let series = p.market.get_series(&series_id).unwrap();
    assert_eq!(series.state, SeriesState::Settled);
    assert_eq!(series.settlement_price, Some(75 * UNIT));
    let settled: i128 = i128::try_from_val(
        &p.env,
        &p.last_event_data(&p.market.address, "settlement_price_set")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(settled, 75 * UNIT);

    // Buyer exercises: (75 - 70) × 2 = 10 USDC, paid from the series tag.
    let before = p.balance(&buyer);
    assert_eq!(p.market.exercise(&buyer, &long), 10 * UNIT);
    assert_eq!(p.balance(&buyer) - before, 10 * UNIT);
    assert_eq!(p.vault.balance_of(&series_id), collateral - 10 * UNIT);

    // Writer reclaims the rest.
    let before = p.balance(&writer);
    assert_eq!(
        p.market.reclaim_collateral(&writer, &short),
        collateral - 10 * UNIT
    );
    assert_eq!(p.balance(&writer) - before, collateral - 10 * UNIT);

    // Nothing left in escrow; both positions closed.
    assert_eq!(p.vault.balance_of(&series_id), 0);
    assert_eq!(p.vault.get_total_escrowed(), 0);
    assert_eq!(p.balance(&p.vault.address), 0);
    assert!(p.market.get_position(&long).unwrap().is_exercised);
    assert!(p.market.get_position(&short).unwrap().is_settled);
    let names = p.event_names(&p.market.address);
    for expected in ["option_written", "option_exercised", "collateral_reclaimed"] {
        assert!(names.contains(&expected.into()), "missing {expected}");
    }
    assert!(p
        .event_names(&p.vault.address)
        .contains(&"withdrawn".into()));
}

#[test]
fn happy_path_otm_put_buyer_gets_nothing_writer_gets_everything() {
    let p = deploy();
    let series_id = p.list_series(OptionType::Put, STRIKE, PREMIUM);
    let (buyer, long) = buy(&p, series_id, UNIT);
    let (writer, short, collateral) = write(&p, series_id, UNIT);
    assert_eq!(collateral, 77 * UNIT); // cash-secured put: 110% of strike

    p.warp_to(p.expiry(series_id));
    p.report(&[80 * UNIT, 81 * UNIT, 79 * UNIT]);
    p.market
        .set_settlement_price_from_oracle(&series_id, &p.oracle.address);

    assert!(p.market.try_exercise(&buyer, &long).is_err()); // NotInTheMoney
    let before = p.balance(&writer);
    assert_eq!(p.market.reclaim_collateral(&writer, &short), collateral);
    assert_eq!(p.balance(&writer) - before, collateral);
    assert_eq!(p.vault.balance_of(&series_id), 0);
    assert_eq!(
        p.market.get_position(&long).unwrap().side,
        PositionSide::Long
    );
}

// ─── failure paths at the seams ─────────────────────────────────────────────

#[test]
fn settlement_before_expiry_is_rejected() {
    let p = deploy();
    let series_id = p.list_series(OptionType::Call, STRIKE, PREMIUM);
    let (buyer, long) = buy(&p, series_id, UNIT);
    write(&p, series_id, UNIT);

    p.warp_to(p.expiry(series_id) - 1);
    p.report(&[75 * UNIT, 75 * UNIT, 75 * UNIT]);
    assert!(p.oracle.get_price(&p.xlm()).is_some());
    assert!(p
        .market
        .try_set_settlement_price_from_oracle(&series_id, &p.oracle.address)
        .is_err()); // SeriesNotExpired
    assert!(p.market.try_exercise(&buyer, &long).is_err());
    let series = p.market.get_series(&series_id).unwrap();
    assert_eq!(series.state, SeriesState::Active);
    assert_eq!(series.settlement_price, None);
}

#[test]
fn settlement_without_quorum_is_rejected_until_feeders_catch_up() {
    let p = deploy();
    p.oracle.set_min_reports(&3);
    let series_id = p.list_series(OptionType::Call, STRIKE, PREMIUM);
    buy(&p, series_id, UNIT);
    write(&p, series_id, UNIT);

    // Two feeders reported a day early and went stale; only one is fresh.
    p.warp_to(p.expiry(series_id) - DAY);
    p.report(&[75 * UNIT, 75 * UNIT]);
    p.warp_to(p.expiry(series_id));
    p.oracle
        .report_price(&p.feeders[2], &p.xlm(), &(500 * UNIT));

    assert_eq!(p.oracle.get_price(&p.xlm()), None);
    assert!(p.oracle.try_aggregate(&p.xlm()).is_err()); // QuorumNotMet
    assert!(p
        .market
        .try_set_settlement_price_from_oracle(&series_id, &p.oracle.address)
        .is_err()); // PriceNotSet
    assert_eq!(
        p.market.get_series(&series_id).unwrap().settlement_price,
        None
    );

    // Once all three are fresh the outlier is filtered — but then only
    // two survive, still short of a quorum of three.
    p.report(&[75 * UNIT, 75 * UNIT]);
    assert_eq!(p.oracle.get_price(&p.xlm()), None);

    // Honest reports from everyone: settles.
    p.report(&[75 * UNIT, 75 * UNIT, 75 * UNIT + 10_000]);
    p.market
        .set_settlement_price_from_oracle(&series_id, &p.oracle.address);
    assert_eq!(
        p.market.get_series(&series_id).unwrap().settlement_price,
        Some(75 * UNIT)
    );
}

#[test]
fn a_withdrawal_against_one_series_tag_cannot_touch_another_series_funds() {
    let p = deploy();
    // Series A: a long but no writer. Series B: fully collateralized.
    let a = p.list_series(OptionType::Call, STRIKE, PREMIUM);
    let b = p.list_series(OptionType::Call, 65 * UNIT, PREMIUM);
    let (buyer, long_a) = buy(&p, a, UNIT);
    buy(&p, b, UNIT);
    let (_, _, collateral_b) = write(&p, b, UNIT);
    assert_eq!(p.vault.balance_of(&a), 0);
    assert_eq!(p.vault.balance_of(&b), collateral_b);
    assert!(p.balance(&p.vault.address) >= 5 * UNIT);

    p.warp_to(p.expiry(a));
    p.report(&[75 * UNIT, 75 * UNIT, 75 * UNIT]);
    p.market
        .set_settlement_price_from_oracle(&a, &p.oracle.address);

    // A's payout would come out of tag A, which is empty — the vault's
    // other tags are not available to it.
    let before = p.balance(&buyer);
    assert!(p.market.try_exercise(&buyer, &long_a).is_err());
    assert_eq!(p.balance(&buyer), before);
    assert_eq!(p.vault.balance_of(&b), collateral_b);

    // Nobody but the vault's admin (the market) can withdraw directly.
    let thief = Address::generate(&p.env);
    p.sign(
        &thief,
        &p.vault.address,
        "withdraw",
        (b, thief.clone(), collateral_b).into_val(&p.env),
    );
    assert!(p.vault.try_withdraw(&b, &thief, &collateral_b).is_err());
    assert_eq!(p.vault.balance_of(&b), collateral_b);
}

#[test]
fn a_rogue_oracle_cannot_settle_and_a_rogue_vault_cannot_take_escrow() {
    let p = deploy();
    let series_id = p.list_series(OptionType::Call, STRIKE, PREMIUM);
    let (buyer, long) = buy(&p, series_id, UNIT);
    p.warp_to(p.expiry(series_id));

    // A second, real price_oracle that the attacker feeds.
    let rogue = price_oracle::PriceOracleClient::new(
        &p.env,
        &p.env.register_contract(None, price_oracle::PriceOracle),
    );
    let attacker = Address::generate(&p.env);
    rogue.initialize(&attacker);
    rogue.add_feeder(&attacker);
    rogue.report_price(&attacker, &p.xlm(), &(10_000 * UNIT));
    assert!(p
        .market
        .try_set_settlement_price_from_oracle(&series_id, &rogue.address)
        .is_err());

    // Cancelled series: escrow only ever goes to the series' own vault.
    let other = p.list_series(OptionType::Put, STRIKE, PREMIUM);
    buy(&p, other, UNIT);
    p.market.cancel_series(&other);
    let rogue_vault = vault::VaultClient::new(&p.env, &p.env.register_contract(None, vault::Vault));
    rogue_vault.initialize(&p.market.address, &p.usdc.address);
    assert!(p
        .market
        .try_escrow_series_to_vault(&rogue_vault.address, &other)
        .is_err());
    p.market.escrow_series_to_vault(&p.vault.address, &other);
    assert!(p.vault.balance_of(&other) > 0);
    let _ = (buyer, long);
}

/// The most expensive cross-contract calls each fit the default Soroban
/// budget: the budget is reset to the default limits right before each
/// one, so exceeding it would fail the call itself.
#[test]
fn the_heaviest_seam_calls_fit_the_default_budget() {
    let p = deploy();
    let series_id = p.list_series(OptionType::Call, STRIKE, PREMIUM);
    let (buyer, long) = buy(&p, series_id, UNIT);

    let measure = |label: &str, f: &dyn Fn()| {
        p.env.budget().reset_default();
        f();
        let cpu = p.env.budget().cpu_instruction_cost();
        let mem = p.env.budget().memory_bytes_cost();
        std::println!("{label}: {cpu} cpu instructions, {mem} memory bytes");
        assert!(cpu > 0);
    };

    let writer = p.user(10_000 * UNIT);
    measure("write_option (market -> vault deposit)", &|| {
        p.market
            .write_option(&writer, &series_id, &UNIT, &(10_000 * UNIT), &0);
    });
    p.warp_to(p.expiry(series_id));
    p.report(&[75 * UNIT, 75 * UNIT, 75 * UNIT]);
    measure("set_settlement_price_from_oracle (market -> oracle)", &|| {
        p.market
            .set_settlement_price_from_oracle(&series_id, &p.oracle.address);
    });
    measure("exercise (market -> vault withdraw)", &|| {
        p.market.exercise(&buyer, &long);
    });
}
