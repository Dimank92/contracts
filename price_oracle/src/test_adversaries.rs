//! Adversarial cross-contract negative tests for price_oracle (issue #115).
//!
//! Each test exercises one cross-contract trust boundary in price_oracle
//! with an adversarial counterparty from `test_adversaries`.
//!
//! # Cross-contract boundaries covered
//!
//! 1. Multisig boundary — every `_via_multisig` entrypoint calls `is_approved(action_id)` on the pinned multisig.
//!    Adversaries: `NeverApproveMultisig`, `AlwaysApproveMultisig`, `ExpiredApprovalMultisig`.
//!
//! 2. Oracle-adapter / multi-source boundary — `oracle_adapter.rs` calls external oracles.
//!    Adversaries: `AlwaysNoneOracle`, `RevertingOracle`, `NegativePriceOracle`.
//!
//! Every `_via_multisig` entrypoint only trusts the multisig pinned with
//! `set_multisig`, so an adversary at any other address is rejected
//! before it is ever called.

#![cfg(test)]

extern crate std;

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, IntoVal, Symbol};

use test_adversaries::multisig::{
    AlwaysApproveMultisig, ExpiredApprovalMultisig, NeverApproveMultisig,
};
use test_adversaries::oracle::{AlwaysNoneOracle, NegativePriceOracle, RevertingOracle};

// ─── helpers ─────────────────────────────────────────────────────────────────

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

/// What a client's `try_` call returns for a `panic_with_error!(Unauthorized)`.
fn unauthorized() -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(crate::Error::Unauthorized as u32)
}

// ─── Multisig boundary ───────────────────────────────────────────────────────

use crate::{PriceOracle, PriceOracleClient};

struct Harness<'a> {
    env: Env,
    client: PriceOracleClient<'a>,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let oracle_id = env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&env, &oracle_id);
    client.initialize(&admin);
    Harness { env, client }
}

/// Registers a never-approve multisig and pins it as the oracle's
/// trusted multisig, so the rejection comes from the approval check
/// rather than the address check.
fn pinned_never_approve(h: &Harness) -> Address {
    let ms_id = h.env.register_contract(None, NeverApproveMultisig);
    h.client.set_multisig(&ms_id);
    ms_id
}

/// `pause_via_multisig` with a never-approve multisig must reject with
/// `Unauthorized` — the call must not pause the oracle.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.pause_via_multisig.**
#[test]
fn pause_via_multisig_with_never_approve_multisig_is_rejected() {
    let h = setup();
    let ms_id = pinned_never_approve(&h);
    let result = h.client.try_pause_via_multisig(&ms_id, &aid(&h.env, 1));
    assert_eq!(result, Err(Ok(unauthorized())));
    assert!(!h.client.is_paused());
}

/// `add_feeder_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.add_feeder_via_multisig.**
#[test]
fn add_feeder_via_multisig_with_never_approve_multisig_is_rejected() {
    let h = setup();
    let ms_id = pinned_never_approve(&h);
    let feeder = Address::generate(&h.env);
    let result = h
        .client
        .try_add_feeder_via_multisig(&ms_id, &aid(&h.env, 1), &feeder);
    assert_eq!(result, Err(Ok(unauthorized())));
    assert!(!h.client.is_feeder(&feeder));
}

/// `remove_feeder_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.remove_feeder_via_multisig.**
#[test]
fn remove_feeder_via_multisig_with_never_approve_multisig_is_rejected() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    let ms_id = pinned_never_approve(&h);
    let result = h
        .client
        .try_remove_feeder_via_multisig(&ms_id, &aid(&h.env, 1), &feeder);
    assert_eq!(result, Err(Ok(unauthorized())));
    assert!(h.client.is_feeder(&feeder));
}

/// `set_max_staleness_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.set_max_staleness_via_multisig.**
#[test]
fn set_max_staleness_via_multisig_with_never_approve_is_rejected() {
    let h = setup();
    let ms_id = pinned_never_approve(&h);
    let result = h
        .client
        .try_set_max_staleness_via_multisig(&ms_id, &aid(&h.env, 1), &7200);
    assert_eq!(result, Err(Ok(unauthorized())));
    assert_eq!(h.client.get_max_staleness(), 3600);
}

/// `set_min_reports_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.set_min_reports_via_multisig.**
#[test]
fn set_min_reports_via_multisig_with_never_approve_is_rejected() {
    let h = setup();
    let ms_id = pinned_never_approve(&h);
    let result = h
        .client
        .try_set_min_reports_via_multisig(&ms_id, &aid(&h.env, 1), &3);
    assert_eq!(result, Err(Ok(unauthorized())));
    assert_eq!(h.client.get_min_reports(), 1);
}

/// `transfer_admin_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.transfer_admin_via_multisig.**
#[test]
fn transfer_admin_via_multisig_with_never_approve_is_rejected() {
    let h = setup();
    let admin = h.client.get_admin();
    let ms_id = pinned_never_approve(&h);
    let attacker = Address::generate(&h.env);
    let result = h
        .client
        .try_transfer_admin_via_multisig(&ms_id, &aid(&h.env, 1), &attacker);
    assert_eq!(result, Err(Ok(unauthorized())));
    assert_eq!(h.client.get_admin(), admin);
}

/// An expired multisig approval must cause `pause_via_multisig` to reject,
/// even if M-of-N approved at some earlier ledger.
///
/// **Issue #115 — Multisig: ExpiredApprovalMultisig at pause_via_multisig.**
#[test]
fn pause_via_multisig_with_expired_approval_is_rejected() {
    let h = setup();
    let ms_id = h.env.register_contract(None, ExpiredApprovalMultisig);
    h.client.set_multisig(&ms_id);
    let result = h.client.try_pause_via_multisig(&ms_id, &aid(&h.env, 1));
    assert_eq!(result, Err(Ok(unauthorized())));
    assert!(!h.client.is_paused());
}

/// An always-approve multisig at an address that was NOT configured as the
/// oracle's trusted multisig must NOT be accepted — by any entrypoint,
/// and also not before any multisig was pinned at all.
///
/// **Issue #115 — Multisig: AlwaysApproveMultisig substitution at price_oracle.**
#[test]
fn always_approve_multisig_at_wrong_address_is_rejected() {
    let h = setup();
    let rogue = h.env.register_contract(None, AlwaysApproveMultisig);
    let feeder = Address::generate(&h.env);
    let id = aid(&h.env, 1);

    // Nothing pinned yet.
    assert_eq!(
        h.client.try_pause_via_multisig(&rogue, &id),
        Err(Ok(unauthorized()))
    );

    // A different multisig pinned.
    let trusted = h.env.register_contract(None, NeverApproveMultisig);
    h.client.set_multisig(&trusted);
    assert_eq!(
        h.client.try_pause_via_multisig(&rogue, &id),
        Err(Ok(unauthorized()))
    );
    assert_eq!(
        h.client.try_add_feeder_via_multisig(&rogue, &id, &feeder),
        Err(Ok(unauthorized()))
    );
    assert_eq!(
        h.client
            .try_transfer_admin_via_multisig(&rogue, &id, &feeder),
        Err(Ok(unauthorized()))
    );
    assert!(!h.client.is_paused());
    assert!(!h.client.is_feeder(&feeder));
}

// ─── Oracle-adapter boundary — LIVE tests ────────────────────────────────────

use crate::oracle_adapter::{AdapterError, MultiSourceOracleAdapter};

/// The multi-source oracle adapter must return `NoValidPrice` (not `Ok(0)`)
/// when both primary and fallback sources return `None`.
///
/// **Issue #115 — Oracle-adapter: both sources return None.**
#[test]
fn oracle_adapter_both_none_sources_return_no_valid_price() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);

    let primary = env.register_contract(None, AlwaysNoneOracle);
    env.invoke_contract::<()>(
        &primary,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.clone().into_val(&env)],
    );

    let fallback = env.register_contract(None, AlwaysNoneOracle);
    env.invoke_contract::<()>(
        &fallback,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.clone().into_val(&env)],
    );

    let result = MultiSourceOracleAdapter::resolve_price(
        &env,
        &primary,
        &fallback,
        Symbol::new(&env, "XLM"),
        3600,
        500, // 5% tolerance in bps
    );
    assert_eq!(
        result,
        Err(AdapterError::NoValidPrice),
        "both-None sources must return NoValidPrice not Ok(0)"
    );
}

/// When the primary oracle returns `None` but the fallback has a fresh price,
/// `resolve_price` must use the fallback rather than failing.
/// When BOTH return None, it must fail.
///
/// This confirms the fallback-path is not silently eaten.
///
/// **Issue #115 — Oracle-adapter: primary None, fallback None → must fail.**
#[test]
fn oracle_adapter_primary_none_fallback_none_fails() {
    let env = Env::default();
    env.mock_all_auths();

    // Both are AlwaysNoneOracle — same as the test above but named separately
    // for documentation clarity: primary and fallback both failing.
    let admin = Address::generate(&env);

    let primary = env.register_contract(None, AlwaysNoneOracle);
    env.invoke_contract::<()>(
        &primary,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.clone().into_val(&env)],
    );
    let fallback = env.register_contract(None, AlwaysNoneOracle);
    env.invoke_contract::<()>(
        &fallback,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.into_val(&env)],
    );

    let result = MultiSourceOracleAdapter::resolve_price(
        &env,
        &primary,
        &fallback,
        Symbol::new(&env, "BTC"),
        3600,
        200,
    );
    assert!(result.is_err(), "both-None must not succeed");
}

/// The adapter must return an error when the negative-price oracle is the
/// only source, since a negative price is invalid for settlement.
///
/// **Issue #115 — Oracle-adapter: NegativePriceOracle as primary source.**
#[test]
fn oracle_adapter_negative_primary_price_must_return_error() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);

    let primary = env.register_contract(None, NegativePriceOracle);
    env.invoke_contract::<()>(
        &primary,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.clone().into_val(&env)],
    );
    let fallback = env.register_contract(None, AlwaysNoneOracle);
    env.invoke_contract::<()>(
        &fallback,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.into_val(&env)],
    );

    let result = MultiSourceOracleAdapter::resolve_price(
        &env,
        &primary,
        &fallback,
        Symbol::new(&env, "XLM"),
        3600,
        500,
    );
    assert!(
        result.is_err(),
        "negative price from oracle must not be returned as Ok"
    );
}

/// A reverting primary must be treated as "no price", letting a healthy
/// fallback win — and failing closed when the fallback has none either.
///
/// **Issue #115 — Oracle-adapter: RevertingOracle as primary source.**
#[test]
fn oracle_adapter_reverting_primary_falls_back_or_fails_closed() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);

    let primary = env.register_contract(None, RevertingOracle);
    let none = env.register_contract(None, AlwaysNoneOracle);
    let real = env.register_contract(None, crate::PriceOracle);
    let real_client = crate::PriceOracleClient::new(&env, &real);
    real_client.initialize(&admin);
    let feeder = Address::generate(&env);
    real_client.add_feeder(&feeder);
    let xlm = Symbol::new(&env, "XLM");
    real_client.report_price(&feeder, &xlm, &1_200_000);

    let quote =
        MultiSourceOracleAdapter::resolve_price(&env, &primary, &real, xlm.clone(), 3600, 500)
            .unwrap();
    assert_eq!(quote.price, 1_200_000);

    let result = MultiSourceOracleAdapter::resolve_price(&env, &primary, &none, xlm, 3600, 500);
    assert_eq!(result, Err(AdapterError::NoValidPrice));
}
