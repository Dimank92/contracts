#![cfg(test)]

extern crate std;

use crate::{PriceOracle, PriceOracleClient};
use multisig::{Multisig, MultisigClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    Address, BytesN, Env, Symbol, TryFromVal,
};

struct Harness<'a> {
    env: Env,
    client: PriceOracleClient<'a>,
    admin: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&env, &contract_id);
    client.initialize(&admin);

    Harness { env, client, admin }
}

#[test]
fn initialize_sets_admin() {
    setup();
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn initialize_twice_panics() {
    let h = setup();
    h.client.initialize(&h.admin);
}

#[test]
#[should_panic] // no auth was mocked at all — require_auth() has nothing to accept
fn initialize_without_any_authorization_panics() {
    // Deliberately skips mock_all_auths(): every other test in this file
    // uses it (mock_all_auths() arms for the env's whole lifetime once
    // called, so there's no way to "unmock" partway through a test to
    // check a LATER call specifically) — this test exists just to
    // confirm require_auth() is actually load-bearing on initialize(),
    // not a no-op, by never arming it in the first place. Mirrors
    // options_market's identically-named test.
    let env = Env::default();
    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&env, &contract_id);
    client.initialize(&admin);
}

// ─── feeder management ───────────────────────────────────────────────────────

#[test]
fn add_feeder_authorizes_and_counts_it() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    assert_eq!(h.client.get_feeder_count(), 0);

    h.client.add_feeder(&feeder);
    assert!(h.client.is_feeder(&feeder));
    assert_eq!(h.client.get_feeder_count(), 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // FeederAlreadyAdded
fn add_feeder_rejects_a_duplicate() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.add_feeder(&feeder);
}

#[test]
fn remove_feeder_revokes_authorization() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);

    h.client.remove_feeder(&feeder);
    assert!(!h.client.is_feeder(&feeder));
    assert_eq!(h.client.get_feeder_count(), 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // FeederNotFound
fn remove_feeder_rejects_an_unknown_address() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    h.client.remove_feeder(&stranger);
}

#[test]
fn is_feeder_is_false_for_an_unauthorized_address() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    assert!(!h.client.is_feeder(&stranger));
}

// ─── report_price ────────────────────────────────────────────────────────────

#[test]
fn report_price_stores_the_feeders_report() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);

    let xlm = Symbol::new(&h.env, "XLM");
    h.client.report_price(&feeder, &xlm, &1_200_000);

    let (price, timestamp) = h.client.get_latest_report(&xlm, &feeder).unwrap();
    assert_eq!(price, 1_200_000);
    assert_eq!(timestamp, h.env.ledger().timestamp());
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // NotAFeeder
fn report_price_rejects_an_unauthorized_reporter() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    h.client
        .report_price(&stranger, &Symbol::new(&h.env, "XLM"), &1_200_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // InvalidPrice
fn report_price_rejects_a_non_positive_price() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client
        .report_price(&feeder, &Symbol::new(&h.env, "XLM"), &0);
}

#[test]
fn report_price_rejects_a_revoked_feeder() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.remove_feeder(&feeder);

    let result = h
        .client
        .try_report_price(&feeder, &Symbol::new(&h.env, "XLM"), &1_200_000);
    assert!(result.is_err());
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // TooManyFeeders
fn add_feeder_rejects_beyond_the_cap() {
    let h = setup();
    for _ in 0..16 {
        h.client.add_feeder(&Address::generate(&h.env));
    }
    assert_eq!(h.client.get_feeder_count(), 16);
    h.client.add_feeder(&Address::generate(&h.env));
}

#[test]
fn get_latest_report_is_none_before_any_report() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    assert!(h
        .client
        .get_latest_report(&Symbol::new(&h.env, "XLM"), &feeder)
        .is_none());
}

// ─── get_price aggregation ───────────────────────────────────────────────────

#[test]
fn get_price_is_none_with_no_feeders() {
    let h = setup();
    assert!(h.client.get_price(&Symbol::new(&h.env, "XLM")).is_none());
}

#[test]
fn get_price_matches_the_single_feeders_report() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    let xlm = Symbol::new(&h.env, "XLM");
    h.client.report_price(&feeder, &xlm, &1_200_000);

    assert_eq!(h.client.get_price(&xlm).unwrap(), 1_200_000);
}

#[test]
fn get_price_is_the_median_across_several_feeders() {
    let h = setup();
    let xlm = Symbol::new(&h.env, "XLM");
    let prices = [1_000_000i128, 1_100_000, 1_050_000];
    for price in prices {
        let feeder = Address::generate(&h.env);
        h.client.add_feeder(&feeder);
        h.client.report_price(&feeder, &xlm, &price);
    }

    assert_eq!(h.client.get_price(&xlm).unwrap(), 1_050_000);
}

#[test]
fn get_price_ignores_a_stale_report() {
    let h = setup();
    let xlm = Symbol::new(&h.env, "XLM");

    let stale_feeder = Address::generate(&h.env);
    h.client.add_feeder(&stale_feeder);
    h.client.report_price(&stale_feeder, &xlm, &999_999_999); // way off, but will go stale

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + h.client.get_max_staleness() + 1);

    let fresh_feeder = Address::generate(&h.env);
    h.client.add_feeder(&fresh_feeder);
    h.client.report_price(&fresh_feeder, &xlm, &1_200_000);

    // Only the fresh feeder's report counts — the stale one is excluded
    // entirely rather than dragging the median toward its outlier value.
    assert_eq!(h.client.get_price(&xlm).unwrap(), 1_200_000);
}

#[test]
fn get_price_ignores_a_revoked_feeders_old_report() {
    let h = setup();
    let xlm = Symbol::new(&h.env, "XLM");

    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.report_price(&feeder, &xlm, &1_200_000);
    h.client.remove_feeder(&feeder);

    // Their report is still in storage (audit trail), but a revoked feeder
    // no longer counts toward the aggregate.
    assert!(h.client.get_latest_report(&xlm, &feeder).is_some());
    assert!(h.client.get_price(&xlm).is_none());
}

// ─── max_staleness ───────────────────────────────────────────────────────────

#[test]
fn max_staleness_defaults_to_one_hour_and_is_configurable() {
    let h = setup();
    assert_eq!(h.client.get_max_staleness(), 3600);

    h.client.set_max_staleness(&7200);
    assert_eq!(h.client.get_max_staleness(), 7200);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // InvalidStaleness
fn set_max_staleness_rejects_zero() {
    let h = setup();
    h.client.set_max_staleness(&0);
}

// ─── min_reports quorum ──────────────────────────────────────────────────────

#[test]
fn min_reports_defaults_to_one_and_is_configurable() {
    let h = setup();
    assert_eq!(h.client.get_min_reports(), 1);

    h.client.set_min_reports(&3);
    assert_eq!(h.client.get_min_reports(), 3);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // InvalidMinReports
fn set_min_reports_rejects_zero() {
    let h = setup();
    h.client.set_min_reports(&0);
}

#[test]
fn get_price_is_none_when_fresh_feeders_fall_short_of_the_quorum() {
    let h = setup();
    h.client.set_min_reports(&2);

    let xlm = Symbol::new(&h.env, "XLM");
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.report_price(&feeder, &xlm, &1_200_000);

    // Only 1 fresh report but the quorum requires 2 — get_price must not
    // fall back to trusting that single feeder's price as the aggregate.
    assert!(h.client.get_price(&xlm).is_none());
}

#[test]
fn get_price_returns_once_the_quorum_is_met() {
    let h = setup();
    h.client.set_min_reports(&2);

    let xlm = Symbol::new(&h.env, "XLM");
    let feeder_a = Address::generate(&h.env);
    let feeder_b = Address::generate(&h.env);
    h.client.add_feeder(&feeder_a);
    h.client.add_feeder(&feeder_b);
    h.client.report_price(&feeder_a, &xlm, &1_000_000);
    assert!(h.client.get_price(&xlm).is_none());

    h.client.report_price(&feeder_b, &xlm, &1_100_000);
    assert!(h.client.get_price(&xlm).is_some());
}

#[test]
fn a_feeder_going_stale_can_drop_below_the_quorum() {
    let h = setup();
    h.client.set_min_reports(&2);

    let xlm = Symbol::new(&h.env, "XLM");
    let feeder_a = Address::generate(&h.env);
    let feeder_b = Address::generate(&h.env);
    h.client.add_feeder(&feeder_a);
    h.client.add_feeder(&feeder_b);
    h.client.report_price(&feeder_a, &xlm, &1_000_000);
    h.client.report_price(&feeder_b, &xlm, &1_100_000);
    assert!(h.client.get_price(&xlm).is_some());

    // feeder_a's report ages out; only feeder_b stays fresh, below quorum.
    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + h.client.get_max_staleness() + 1);
    h.client.report_price(&feeder_b, &xlm, &1_100_000);

    assert!(h.client.get_price(&xlm).is_none());
}

// ─── transfer_admin ─────────────────────────────────────────────────────────

#[test]
fn transfer_admin_hands_off_control() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);

    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);
    assert_eq!(h.client.get_admin(), new_admin);

    // Admin-gated calls still work, now authorized against the new admin.
    h.client.add_feeder(&Address::generate(&h.env));
}

// ─── pause / unpause ────────────────────────────────────────────────────────

#[test]
fn pause_blocks_mutations_unpause_restores_them() {
    let h = setup();
    assert!(!h.client.is_paused());

    h.client.pause();
    assert!(h.client.is_paused());

    h.client.unpause();
    assert!(!h.client.is_paused());
    h.client.add_feeder(&Address::generate(&h.env));
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // ContractPaused
fn add_feeder_is_rejected_while_paused() {
    let h = setup();
    h.client.pause();
    h.client.add_feeder(&Address::generate(&h.env));
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // ContractPaused
fn report_price_is_rejected_while_paused() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.pause();
    h.client
        .report_price(&feeder, &Symbol::new(&h.env, "XLM"), &1_200_000);
}

/// A pause must not hide the last-known price from callers still reading
/// it — get_price stays live even while mutations are frozen.
#[test]
fn pause_does_not_block_get_price() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    let xlm = Symbol::new(&h.env, "XLM");
    h.client.report_price(&feeder, &xlm, &1_200_000);

    h.client.pause();
    assert_eq!(h.client.get_price(&xlm).unwrap(), 1_200_000);
}

// ─── events ──────────────────────────────────────────────────────────────────

#[test]
fn add_feeder_emits_a_feeder_added_event() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);

    let events = h.env.events().all();
    let (contract_id, _topics, data) = events.last().unwrap();
    assert_eq!(contract_id, h.client.address);
    assert_eq!(Address::try_from_val(&h.env, &data).unwrap(), feeder);
}

#[test]
fn remove_feeder_emits_a_feeder_removed_event() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.remove_feeder(&feeder);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(Address::try_from_val(&h.env, &data).unwrap(), feeder);
}

#[test]
fn report_price_emits_a_price_reported_event() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    let xlm = Symbol::new(&h.env, "XLM");

    h.client.report_price(&feeder, &xlm, &1_200_000);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 1_200_000);

    // feeder and symbol live in topics, not data — an indexer filtering
    // "reports from this feeder" or "reports for this symbol" reads
    // them from here.
    let topic_feeder = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let topic_symbol = Symbol::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(topic_feeder, feeder);
    assert_eq!(topic_symbol, xlm);
}

#[test]
fn transfer_admin_emits_an_admin_transferred_event_with_old_and_new_admin() {
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
fn set_max_staleness_emits_a_max_staleness_updated_event_with_the_new_value() {
    let h = setup();
    h.client.set_max_staleness(&7200);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(u64::try_from_val(&h.env, &data).unwrap(), 7200);
}

#[test]
fn set_min_reports_emits_a_min_reports_updated_event_with_the_new_value() {
    let h = setup();
    h.client.set_min_reports(&3);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(u32::try_from_val(&h.env, &data).unwrap(), 3);
}

#[test]
fn pause_and_unpause_each_emit_their_own_event() {
    let h = setup();
    let events_before = h.env.events().all().len();

    h.client.pause();
    h.client.unpause();

    let events_after = h.env.events().all().len();
    assert_eq!(events_after - events_before, 2);
}

// ─── cross-contract: pause_via_multisig / transfer_admin_via_multisig ─────

/// Deploys a real multisig contract in the SAME Env as the price
/// oracle under test, with a 2-of-3 threshold.
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
fn pause_via_multisig_pauses_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 1);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    assert!(!h.client.is_paused());
    h.client.pause_via_multisig(&multisig_id, &action_id);
    assert!(h.client.is_paused());
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn pause_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    multisig_client.approve(&signers[0], &aid(&h.env, 1)); // only 1 of 3

    h.client.pause_via_multisig(&multisig_id, &aid(&h.env, 1));
}

#[test]
fn unpause_via_multisig_unpauses_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    h.client.pause();
    assert!(h.client.is_paused());

    let action_id = aid(&h.env, 3);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client.unpause_via_multisig(&multisig_id, &action_id);
    assert!(!h.client.is_paused());
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn unpause_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client.pause();
    h.client
        .unpause_via_multisig(&multisig_id, &aid(&h.env, 99));
}

#[test]
fn transfer_admin_via_multisig_hands_off_control_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 2);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let new_admin = Address::generate(&h.env);
    h.client
        .transfer_admin_via_multisig(&multisig_id, &action_id, &new_admin);
    assert_eq!(h.client.get_admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn transfer_admin_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);
    let new_admin = Address::generate(&h.env);

    h.client
        .transfer_admin_via_multisig(&multisig_id, &aid(&h.env, 99), &new_admin);
}

#[test]
fn set_max_staleness_via_multisig_applies_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 4);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .set_max_staleness_via_multisig(&multisig_id, &action_id, &7200);
    assert_eq!(h.client.get_max_staleness(), 7200);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn set_max_staleness_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client
        .set_max_staleness_via_multisig(&multisig_id, &aid(&h.env, 99), &7200);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // InvalidStaleness
fn set_max_staleness_via_multisig_still_rejects_zero_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 5);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .set_max_staleness_via_multisig(&multisig_id, &action_id, &0);
}

#[test]
fn set_min_reports_via_multisig_applies_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 6);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .set_min_reports_via_multisig(&multisig_id, &action_id, &3);
    assert_eq!(h.client.get_min_reports(), 3);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn set_min_reports_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client
        .set_min_reports_via_multisig(&multisig_id, &aid(&h.env, 99), &3);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // InvalidMinReports
fn set_min_reports_via_multisig_still_rejects_zero_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 7);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .set_min_reports_via_multisig(&multisig_id, &action_id, &0);
}

// ─── cross-contract: add_feeder_via_multisig / remove_feeder_via_multisig ──

#[test]
fn add_feeder_via_multisig_authorizes_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 6);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let feeder = Address::generate(&h.env);
    h.client
        .add_feeder_via_multisig(&multisig_id, &action_id, &feeder);
    assert!(h.client.is_feeder(&feeder));
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn add_feeder_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);
    let feeder = Address::generate(&h.env);

    h.client
        .add_feeder_via_multisig(&multisig_id, &aid(&h.env, 99), &feeder);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // FeederAlreadyAdded
fn add_feeder_via_multisig_still_rejects_a_duplicate_once_approved() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 7);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .add_feeder_via_multisig(&multisig_id, &action_id, &feeder);
}

#[test]
fn remove_feeder_via_multisig_revokes_once_approved() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 8);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .remove_feeder_via_multisig(&multisig_id, &action_id, &feeder);
    assert!(!h.client.is_feeder(&feeder));
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn remove_feeder_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client
        .remove_feeder_via_multisig(&multisig_id, &aid(&h.env, 99), &feeder);
}

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

// ─── TTL policy (issue #98) and archival restore (issue #99) ────────────────

fn advance_ledgers(env: &Env, ledgers: u32) {
    use soroban_sdk::testutils::Ledger as _;
    env.ledger().with_mut(|l| l.sequence_number += ledgers);
}

/// Whether `key` (a persistent entry, or the instance when `None`) of
/// `contract` is still live. Accessing an archived entry through a
/// client aborts the test rather than returning an error, so archival is
/// checked straight against the ledger storage instead.
fn is_live(env: &Env, contract: &Address, key: Option<crate::types::DataKey>) -> bool {
    use soroban_sdk::xdr::{LedgerKey, ScAddress, ScVal};
    use soroban_sdk::IntoVal;
    let key = match key {
        Some(key) => {
            let val: soroban_sdk::Val = key.into_val(env);
            ScVal::try_from_val(env, &val).unwrap()
        }
        None => ScVal::LedgerKeyContractInstance,
    };
    let contract = ScAddress::from(contract);
    let seq = env.ledger().sequence();
    env.host()
        .with_mut_storage(|storage| {
            for (ledger_key, entry) in storage.map.clone() {
                if let LedgerKey::ContractData(data) = ledger_key.as_ref() {
                    if data.contract == contract && data.key == key {
                        return Ok(
                            matches!(entry, Some((_, Some(live_until))) if live_until >= seq),
                        );
                    }
                }
            }
            Ok(false)
        })
        .unwrap()
}

/// Simulates a `RestoreFootprint` operation over every archived
/// persistent entry: like the real operation, it brings the entry back
/// with its stored value untouched and a fresh
/// `min_persistent_entry_ttl` lifetime.
fn restore_archived(env: &Env) {
    use soroban_sdk::testutils::Ledger as _;
    use soroban_sdk::xdr::{ContractDataDurability, LedgerKey};
    let seq = env.ledger().sequence();
    let live_until = seq + env.ledger().get().min_persistent_entry_ttl - 1;
    let budget = env.host().budget_cloned();
    env.host()
        .with_mut_storage(|storage| {
            for (key, entry) in storage.map.clone() {
                let Some((entry, Some(old_live_until))) = entry else {
                    continue;
                };
                if old_live_until >= seq {
                    continue;
                }
                if let LedgerKey::ContractData(data) = key.as_ref() {
                    if data.durability == ContractDataDurability::Temporary {
                        continue;
                    }
                }
                storage.put(&key, &entry, Some(live_until), &budget)?;
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn ttl_policy_keeps_read_prices_live() {
    use crate::{ttl, types::DataKey};
    let h = setup();
    let feeder = Address::generate(&h.env);
    let xlm = Symbol::new(&h.env, "XLM");
    h.client.add_feeder(&feeder);
    h.client.report_price(&feeder, &xlm, &1_000_000);
    for _ in 0..5 {
        advance_ledgers(&h.env, 20 * ttl::DAY_IN_LEDGERS);
        h.client.get_latest_report(&xlm, &feeder);
    }
    assert!(is_live(
        &h.env,
        &h.client.address,
        Some(DataKey::PriceReport(xlm.clone(), feeder.clone()))
    ));
    assert_eq!(
        h.client.get_latest_report(&xlm, &feeder).unwrap().0,
        1_000_000
    );
}

#[test]
fn archived_price_report_restores_identically() {
    use crate::{ttl, types::DataKey};
    let h = setup();
    let feeder = Address::generate(&h.env);
    let xlm = Symbol::new(&h.env, "XLM");
    h.client.add_feeder(&feeder);
    h.client.report_price(&feeder, &xlm, &1_000_000);
    let before = h.client.get_price(&xlm);
    for _ in 0..5 {
        advance_ledgers(&h.env, 20 * ttl::DAY_IN_LEDGERS);
        h.client.is_paused();
    }
    assert!(!is_live(
        &h.env,
        &h.client.address,
        Some(DataKey::PriceReport(xlm.clone(), feeder.clone()))
    ));

    restore_archived(&h.env);

    assert_eq!(h.client.get_price(&xlm), before);
    assert_eq!(
        h.client.get_latest_report(&xlm, &feeder).unwrap().0,
        1_000_000
    );
}

#[test]
fn archived_instance_restores_feeders_and_admin() {
    use crate::ttl;
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    advance_ledgers(&h.env, ttl::INSTANCE_BUMP_AMOUNT + 1);
    assert!(!is_live(&h.env, &h.client.address, None));

    restore_archived(&h.env);

    assert_eq!(h.client.get_admin(), h.admin);
    assert!(h.client.is_feeder(&feeder));
}
