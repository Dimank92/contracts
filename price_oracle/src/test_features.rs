//! TWAP / round history (#251) and slash dispute window (#250) tests.

#![cfg(test)]

extern crate std;

use crate::{
    round_history::MAX_PRICE, BondStatus, PriceOracle, PriceOracleClient, Resolution,
    SlashDestination,
};
use soroban_sdk::{
    testutils::{Address as _, Ledger, MockAuth, MockAuthInvoke},
    Address, Bytes, Env, IntoVal, Symbol,
};

const START: u32 = 1_000;

struct H<'a> {
    env: Env,
    client: PriceOracleClient<'a>,
    xlm: Symbol,
}

fn setup<'a>() -> H<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.sequence_number = START);
    let admin = Address::generate(&env);
    let client = PriceOracleClient::new(&env, &env.register_contract(None, PriceOracle));
    client.initialize(&admin);
    let xlm = Symbol::new(&env, "XLM");
    H { env, client, xlm }
}

impl H<'_> {
    fn feeder(&self) -> Address {
        let f = Address::generate(&self.env);
        self.client.add_feeder(&f);
        f
    }
    fn step(&self, ledgers: u32) {
        self.env.ledger().with_mut(|l| l.sequence_number += ledgers);
    }
    fn bonded_feeder(&self, amount: i128) -> Address {
        let f = self.feeder();
        self.client.deposit_bond(&f, &amount);
        f
    }
    fn window(&self) -> u32 {
        self.client.get_dispute_window()
    }
}

// ─── TWAP and round history ──────────────────────────────────────────────────

#[test]
fn twap_over_equal_and_unequal_round_durations() {
    let h = setup();
    let f = h.feeder();
    h.client.report_price(&f, &h.xlm, &1_000); // round 1 @ START
    h.step(10);
    h.client.report_price(&f, &h.xlm, &2_000); // round 2 @ START+10
    h.step(10); // now = START+20

    assert_eq!(h.client.twap(&h.xlm, &20), Some(1_500)); // equal durations
    assert_eq!(h.client.twap(&h.xlm, &10), Some(2_000));
    assert_eq!(
        h.client.twap(&h.xlm, &15),
        Some((1_000 * 5 + 2_000 * 10) / 15)
    );

    h.step(20); // now = START+40: durations 10 / 30 on a 40-ledger window
    assert_eq!(
        h.client.twap(&h.xlm, &40),
        Some((1_000 * 10 + 2_000 * 30) / 40)
    );
}

#[test]
fn twap_is_none_without_rounds_or_for_a_zero_window() {
    let h = setup();
    assert_eq!(h.client.twap(&h.xlm, &10), None);
    let f = h.feeder();
    h.client.report_price(&f, &h.xlm, &1_000);
    assert_eq!(h.client.twap(&h.xlm, &0), None);
}

#[test]
fn single_round_symbol_collapses_the_window_to_that_round() {
    let h = setup();
    let f = h.feeder();
    h.client.report_price(&f, &h.xlm, &1_000);
    // Zero elapsed: the spot price.
    assert_eq!(h.client.twap(&h.xlm, &500), Some(1_000));
    h.step(7);
    // Window longer than history, but nothing was ever pruned.
    let d = h.client.twap_data(&h.xlm, &500).unwrap();
    assert_eq!(d.twap, 1_000);
    assert_eq!(d.window_ledgers, 7);
}

#[test]
fn a_gap_without_reports_carries_the_last_price_and_is_reported() {
    let h = setup();
    let f = h.feeder();
    h.client.report_price(&f, &h.xlm, &1_000);
    h.step(10);
    h.client.report_price(&f, &h.xlm, &3_000);
    h.step(30); // silent gap
    let d = h.client.twap_data(&h.xlm, &40).unwrap();
    assert_eq!(d.gap_ledgers, 30);
    assert_eq!(d.twap, (1_000 * 10 + 3_000 * 30) / 40);
}

#[test]
fn a_second_report_in_the_same_ledger_replaces_the_round_price() {
    let h = setup();
    let (a, b) = (h.feeder(), h.feeder());
    h.client.set_min_reports(&2);
    h.client.report_price(&a, &h.xlm, &1_000);
    h.client.report_price(&b, &h.xlm, &1_000);
    h.client.report_price(&b, &h.xlm, &3_000);
    assert_eq!(h.client.latest_round(&h.xlm).unwrap().round_id, 1);
    assert_eq!(h.client.latest_round(&h.xlm).unwrap().price, 2_000);
}

/// 300 rounds, one per ledger: only the last 256 are retained.
fn pruned(h: &H) {
    h.env.budget().reset_unlimited();
    let f = h.feeder();
    for i in 0..300u32 {
        h.client.report_price(&f, &h.xlm, &(1_000 + i128::from(i)));
        h.step(1);
    }
}

#[test]
fn window_at_the_retention_boundary_is_served_and_just_past_it_is_none() {
    let h = setup();
    pruned(&h);
    let latest = h.client.latest_round(&h.xlm).unwrap();
    assert_eq!(latest.round_id, 300);
    assert!(h.client.get_round(&h.xlm, &44).is_none()); // pruned
    let oldest = h.client.get_round(&h.xlm, &45).unwrap();

    let now = START + 300;
    let boundary = now - oldest.ledger;
    assert!(h.client.twap(&h.xlm, &boundary).is_some());
    assert_eq!(h.client.twap(&h.xlm, &(boundary + 1)), None);
    // Oversized windows can never read pruned data or unbounded storage.
    assert_eq!(h.client.twap(&h.xlm, &u32::MAX), None);
}

#[test]
fn twap_stays_within_the_min_and_max_of_the_rounds_it_spans() {
    let h = setup();
    let f = h.feeder();
    let mut seed = 12_345u64;
    let (mut lo, mut hi) = (i128::MAX, 0i128);
    for _ in 0..60 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let price = 1_000 + i128::from((seed >> 33) % 9_000);
        let gap = 1 + ((seed >> 20) % 7) as u32;
        h.client.report_price(&f, &h.xlm, &price);
        lo = lo.min(price);
        hi = hi.max(price);
        h.step(gap);
        let t = h.client.twap(&h.xlm, &1_000).unwrap();
        assert!(t >= lo && t <= hi, "twap {t} outside [{lo}, {hi}]");
    }
}

#[test]
fn rounds_are_individually_readable() {
    let h = setup();
    let f = h.feeder();
    h.client.report_price(&f, &h.xlm, &1_000);
    h.step(5);
    h.client.report_price(&f, &h.xlm, &2_000);
    let r2 = h.client.get_round(&h.xlm, &2).unwrap();
    assert_eq!(
        (r2.price, r2.ledger, r2.cumulative),
        (2_000, START + 5, 5_000)
    );
    assert_eq!(h.client.latest_round(&h.xlm), Some(r2));
    assert!(h.client.get_round(&h.xlm, &3).is_none());
}

#[test]
fn extreme_price_over_a_huge_gap_cannot_overflow() {
    let h = setup();
    let f = h.feeder();
    h.client.report_price(&f, &h.xlm, &MAX_PRICE);
    h.step(400_000);
    h.client.report_price(&f, &h.xlm, &MAX_PRICE);
    assert!(h.client.twap(&h.xlm, &1_000).is_some());
    assert!(h
        .client
        .try_report_price(&f, &h.xlm, &(MAX_PRICE + 1))
        .is_err());
}

#[test]
fn twap_query_stays_within_a_ledger_budget() {
    let h = setup();
    pruned(&h);
    h.env.budget().reset_default();
    h.client.twap(&h.xlm, &200).unwrap();
    assert!(h.env.budget().cpu_instruction_cost() < 5_000_000);
}

// ─── Dispute window ──────────────────────────────────────────────────────────

#[test]
fn a_slash_enters_disputed_and_cannot_resolve_before_the_window_closes() {
    let h = setup();
    let f = h.bonded_feeder(1_000);
    h.client.propose_slash(&f, &7, &400);
    let bond = h.client.get_bond(&f).unwrap();
    assert_eq!(bond.amount, 600);
    assert!(matches!(bond.status, BondStatus::Disputed(d) if d.amount == 400 && d.report_id == 7));

    assert!(h
        .client
        .try_resolve_dispute(&f, &Resolution::Slash)
        .is_err());
    h.step(h.window() - 1);
    assert!(h
        .client
        .try_resolve_dispute(&f, &Resolution::Slash)
        .is_err());
    h.step(1);
    h.client.resolve_dispute(&f, &Resolution::Slash);

    let bond = h.client.get_bond(&f).unwrap();
    assert_eq!((bond.amount, bond.status), (600, BondStatus::Active));
    assert_eq!(h.client.get_slashed_totals(), (400, 0));
}

#[test]
fn restore_returns_the_bond_and_reward_pool_destination_is_honoured() {
    let h = setup();
    let f = h.bonded_feeder(1_000);
    h.client.propose_slash(&f, &1, &300);
    h.step(h.window());
    h.client.resolve_dispute(&f, &Resolution::Restore);
    assert_eq!(h.client.get_bond(&f).unwrap().amount, 1_000);
    assert_eq!(h.client.get_slashed_totals(), (0, 0));

    h.client
        .set_slash_destination(&SlashDestination::RewardPool);
    h.client.propose_slash(&f, &2, &250);
    h.step(h.window());
    h.client.resolve_dispute(&f, &Resolution::Slash);
    assert_eq!(h.client.get_slashed_totals(), (0, 250));
}

#[test]
fn a_report_cannot_be_slashed_twice_and_only_one_dispute_is_open_at_a_time() {
    let h = setup();
    let f = h.bonded_feeder(1_000);
    h.client.propose_slash(&f, &1, &100);
    assert!(h.client.try_propose_slash(&f, &2, &100).is_err()); // already disputed
    h.step(h.window());
    h.client.resolve_dispute(&f, &Resolution::Restore);
    assert!(h.client.try_propose_slash(&f, &1, &100).is_err()); // same report
    h.client.propose_slash(&f, &2, &100);
}

#[test]
fn slash_amount_is_bounded_by_the_active_bond() {
    let h = setup();
    let f = h.bonded_feeder(100);
    assert!(h.client.try_propose_slash(&f, &1, &101).is_err());
    assert!(h.client.try_propose_slash(&f, &1, &0).is_err());
    let nobody = Address::generate(&h.env);
    assert!(h.client.try_propose_slash(&nobody, &1, &1).is_err());
}

#[test]
fn a_feeder_removed_mid_dispute_still_resolves() {
    let h = setup();
    let f = h.bonded_feeder(1_000);
    h.client.propose_slash(&f, &1, &400);
    h.client.remove_feeder(&f);
    h.step(h.window());
    h.client.resolve_dispute(&f, &Resolution::Restore);
    let bond = h.client.get_bond(&f).unwrap();
    assert_eq!((bond.amount, bond.status), (1_000, BondStatus::Removed));
    h.client.add_feeder(&f);
    assert_eq!(h.client.get_bond(&f).unwrap().status, BondStatus::Active);
}

#[test]
fn evidence_is_bounded_and_only_accepted_inside_the_window() {
    let h = setup();
    let f = h.bonded_feeder(1_000);
    let ev = |n: u32| Bytes::from_array(&h.env, &[7u8; 64]).slice(0..n);
    assert!(h.client.try_submit_evidence(&f, &ev(1)).is_err()); // no dispute yet

    h.client.propose_slash(&f, &1, &100);
    assert!(h
        .client
        .try_submit_evidence(&f, &Bytes::new(&h.env))
        .is_err());
    assert!(h
        .client
        .try_submit_evidence(&f, &Bytes::from_array(&h.env, &[1u8; 65]))
        .is_err());
    h.client.submit_evidence(&f, &ev(64));
    match h.client.get_bond(&f).unwrap().status {
        BondStatus::Disputed(d) => assert_eq!(d.evidence, Some(ev(64))),
        _ => panic!("not disputed"),
    }
    h.step(h.window());
    assert!(h.client.try_submit_evidence(&f, &ev(8)).is_err());
}

#[test]
fn a_disputed_feeder_is_excluded_from_the_quorum_and_cannot_report() {
    let h = setup();
    h.client.set_min_reports(&2);
    let a = h.bonded_feeder(1_000);
    let b = h.feeder();
    h.client.report_price(&a, &h.xlm, &1_000);
    h.client.report_price(&b, &h.xlm, &1_100);
    assert!(h.client.get_price(&h.xlm).is_some());

    h.client.propose_slash(&a, &1, &100);
    assert_eq!(h.client.get_price(&h.xlm), None);
    assert!(h.client.try_report_price(&a, &h.xlm, &1_000).is_err());

    h.step(h.window());
    h.client.resolve_dispute(&a, &Resolution::Restore);
    assert!(h.client.get_price(&h.xlm).is_some());
}

#[test]
fn an_unarbitrated_dispute_can_be_finalized_by_anyone_after_the_grace_period() {
    let h = setup();
    let f = h.bonded_feeder(1_000);
    h.client.propose_slash(&f, &1, &400);
    h.step(h.window());
    assert!(h.client.try_finalize_dispute(&f).is_err());
    h.step(crate::feeder_staking::FINALIZE_GRACE_LEDGERS);
    h.client.finalize_dispute(&f);
    let bond = h.client.get_bond(&f).unwrap();
    assert_eq!((bond.amount, bond.status), (1_000, BondStatus::Active));
    assert!(h.client.try_finalize_dispute(&f).is_err());
}

#[test]
fn dispute_window_must_be_positive() {
    let h = setup();
    assert!(h.client.try_set_dispute_window(&0).is_err());
    h.client.set_dispute_window(&50);
    assert_eq!(h.client.get_dispute_window(), 50);
}

/// A malicious feeder tries to slash and then restore itself to escape
/// accountability; only the admin's signature is accepted for either.
#[test]
fn a_feeder_cannot_self_slash_or_self_resolve() {
    let h = setup();
    let f = h.bonded_feeder(1_000);
    h.client.propose_slash(&f, &1, &100);
    h.step(h.window());

    let as_feeder = |fn_name: &'static str, args| {
        h.env.mock_auths(&[MockAuth {
            address: &f,
            invoke: &MockAuthInvoke {
                contract: &h.client.address,
                fn_name,
                args,
                sub_invokes: &[],
            },
        }]);
    };
    as_feeder(
        "resolve_dispute",
        (f.clone(), Resolution::Restore).into_val(&h.env),
    );
    assert!(h
        .client
        .try_resolve_dispute(&f, &Resolution::Restore)
        .is_err());
    as_feeder("propose_slash", (f.clone(), 9u64, 1i128).into_val(&h.env));
    assert!(h.client.try_propose_slash(&f, &9, &1).is_err());
}
