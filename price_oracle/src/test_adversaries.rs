//! Adversarial cross-contract negative tests for price_oracle (issue #115).
//!
//! Each test exercises one cross-contract trust boundary in price_oracle
//! with an adversarial counterparty from `test_adversaries`.
//!
//! # Cross-contract boundaries covered
//!
//! 1. Multisig boundary — every `_via_multisig` entrypoint goes through
//!    `zenith_common::require_admin_or_multisig`, which only accepts the
//!    trusted multisig and calls its `is_approved(action_id)`.
//!    Adversaries: `NeverApproveMultisig`, `AlwaysApproveMultisig`,
//!    `ExpiredApprovalMultisig`.
//!
//! 2. Oracle-adapter / multi-source boundary — `oracle_adapter.rs` calls
//!    external oracles' `get_price`.
//!    Adversaries: `AlwaysNoneOracle`, `RevertingOracle`, `NegativePriceOracle`.

#![cfg(test)]

extern crate std;

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, IntoVal, Symbol};

use crate::error::Error;
use crate::{PriceOracle, PriceOracleClient};
use test_adversaries::multisig::{
    AlwaysApproveMultisig, ExpiredApprovalMultisig, NeverApproveMultisig,
};
use test_adversaries::oracle::{AlwaysNoneOracle, NegativePriceOracle, RevertingOracle};

// ─── helpers ─────────────────────────────────────────────────────────────────

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

fn oracle<'a>(env: &Env) -> PriceOracleClient<'a> {
    env.mock_all_auths();
    let client = PriceOracleClient::new(env, &env.register_contract(None, PriceOracle));
    client.initialize(&Address::generate(env));
    client
}

/// An oracle whose trusted multisig is `multisig`.
fn oracle_trusting<'a>(env: &Env, multisig: &Address) -> PriceOracleClient<'a> {
    let client = oracle(env);
    client.set_multisig(multisig);
    client
}

fn unauthorized() -> Result<soroban_sdk::Error, soroban_sdk::InvokeError> {
    Ok(Error::Unauthorized.into())
}

// ─── Multisig boundary ───────────────────────────────────────────────────────

/// `pause_via_multisig` with a never-approve multisig must reject with
/// `Unauthorized` — the call must not pause the oracle.
#[test]
fn pause_via_multisig_with_never_approve_multisig_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, NeverApproveMultisig);
    let client = oracle_trusting(&env, &ms);
    assert_eq!(
        client.try_pause_via_multisig(&ms, &aid(&env, 1)),
        Err(unauthorized())
    );
    assert!(!client.is_paused());
}

#[test]
fn add_feeder_via_multisig_with_never_approve_multisig_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, NeverApproveMultisig);
    let client = oracle_trusting(&env, &ms);
    let feeder = Address::generate(&env);
    assert_eq!(
        client.try_add_feeder_via_multisig(&ms, &aid(&env, 1), &feeder),
        Err(unauthorized())
    );
    assert!(!client.is_feeder(&feeder));
}

#[test]
fn remove_feeder_via_multisig_with_never_approve_multisig_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, NeverApproveMultisig);
    let client = oracle_trusting(&env, &ms);
    let feeder = Address::generate(&env);
    client.add_feeder(&feeder);
    assert_eq!(
        client.try_remove_feeder_via_multisig(&ms, &aid(&env, 1), &feeder),
        Err(unauthorized())
    );
    assert!(client.is_feeder(&feeder));
}

#[test]
fn set_max_staleness_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, NeverApproveMultisig);
    let client = oracle_trusting(&env, &ms);
    assert_eq!(
        client.try_set_max_staleness_via_multisig(&ms, &aid(&env, 1), &60),
        Err(unauthorized())
    );
    assert_eq!(client.get_max_staleness(), 3600);
}

#[test]
fn set_min_reports_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, NeverApproveMultisig);
    let client = oracle_trusting(&env, &ms);
    assert_eq!(
        client.try_set_min_reports_via_multisig(&ms, &aid(&env, 1), &3),
        Err(unauthorized())
    );
    assert_eq!(client.get_min_reports(), 1);
}

#[test]
fn transfer_admin_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, NeverApproveMultisig);
    let client = oracle_trusting(&env, &ms);
    let attacker = Address::generate(&env);
    assert_eq!(
        client.try_transfer_admin_via_multisig(&ms, &aid(&env, 1), &attacker),
        Err(unauthorized())
    );
    assert_eq!(client.get_pending_admin(), None);
}

/// An expired multisig approval must cause `pause_via_multisig` to
/// reject: the consumer asks the multisig on every call rather than
/// trusting an earlier answer.
#[test]
fn pause_via_multisig_with_expired_approval_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, ExpiredApprovalMultisig);
    let client = oracle_trusting(&env, &ms);
    assert_eq!(
        client.try_pause_via_multisig(&ms, &aid(&env, 1)),
        Err(unauthorized())
    );
    assert!(!client.is_paused());
}

/// An always-approve multisig at an address that was NOT configured as the
/// oracle's trusted multisig must NOT be accepted.
#[test]
fn always_approve_multisig_at_wrong_address_is_rejected() {
    let env = Env::default();
    let trusted = env.register_contract(None, NeverApproveMultisig);
    let client = oracle_trusting(&env, &trusted);
    let rogue = env.register_contract(None, AlwaysApproveMultisig);
    let attacker = Address::generate(&env);
    assert_eq!(
        client.try_transfer_admin_via_multisig(&rogue, &aid(&env, 1), &attacker),
        Err(unauthorized())
    );
    assert_eq!(
        client.try_pause_via_multisig(&rogue, &aid(&env, 1)),
        Err(unauthorized())
    );
    assert!(!client.is_paused());
}

/// With no trusted multisig configured, every `_via_multisig` path is
/// closed — even to an always-approve multisig.
#[test]
fn multisig_paths_are_closed_until_a_multisig_is_trusted() {
    let env = Env::default();
    let client = oracle(&env);
    let rogue = env.register_contract(None, AlwaysApproveMultisig);
    assert_eq!(
        client.try_pause_via_multisig(&rogue, &aid(&env, 1)),
        Err(unauthorized())
    );
}

/// Even the trusted multisig's approval only authorizes one call: the
/// same action id can't be replayed for a second admin action.
#[test]
fn an_approval_cannot_be_replayed() {
    let env = Env::default();
    let ms = env.register_contract(None, AlwaysApproveMultisig);
    let client = oracle_trusting(&env, &ms);
    let id = aid(&env, 7);
    client.pause_via_multisig(&ms, &id);
    client.unpause();
    assert_eq!(client.try_pause_via_multisig(&ms, &id), Err(unauthorized()));
    assert!(!client.is_paused());
}

// ─── Oracle-adapter boundary ─────────────────────────────────────────────────

use crate::oracle_adapter::{AdapterError, MultiSourceOracleAdapter};

fn adversary_oracle(env: &Env, contract: Address) -> Address {
    let admin = Address::generate(env);
    env.invoke_contract::<()>(
        &contract,
        &Symbol::new(env, "initialize"),
        soroban_sdk::vec![env, admin.into_val(env)],
    );
    contract
}

/// The multi-source oracle adapter must return `NoValidPrice` (not `Ok(0)`)
/// when both primary and fallback sources return `None`.
#[test]
fn oracle_adapter_both_none_sources_return_no_valid_price() {
    let env = Env::default();
    env.mock_all_auths();
    let primary = adversary_oracle(&env, env.register_contract(None, AlwaysNoneOracle));
    let fallback = adversary_oracle(&env, env.register_contract(None, AlwaysNoneOracle));

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

/// A source that reverts is treated as having no price, not as a fatal
/// error that takes the fallback down with it.
#[test]
fn oracle_adapter_reverting_primary_with_no_fallback_fails_cleanly() {
    let env = Env::default();
    env.mock_all_auths();
    let primary = adversary_oracle(&env, env.register_contract(None, RevertingOracle));
    let fallback = adversary_oracle(&env, env.register_contract(None, AlwaysNoneOracle));

    let result = MultiSourceOracleAdapter::resolve_price(
        &env,
        &primary,
        &fallback,
        Symbol::new(&env, "BTC"),
        3600,
        200,
    );
    assert_eq!(result, Err(AdapterError::NoValidPrice));
}

/// The adapter must return an error when the negative-price oracle is the
/// only source, since a negative price is invalid for settlement.
#[test]
fn oracle_adapter_negative_primary_price_must_return_error() {
    let env = Env::default();
    env.mock_all_auths();
    let primary = adversary_oracle(&env, env.register_contract(None, NegativePriceOracle));
    let fallback = adversary_oracle(&env, env.register_contract(None, AlwaysNoneOracle));

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

/// A real price_oracle works as an adapter source.
#[test]
fn oracle_adapter_reads_a_real_price_oracle() {
    let env = Env::default();
    let client = oracle(&env);
    let feeder = Address::generate(&env);
    let xlm = Symbol::new(&env, "XLM");
    client.add_feeder(&feeder);
    client.report_price(&feeder, &xlm, &12_345_678);
    let fallback = adversary_oracle(&env, env.register_contract(None, AlwaysNoneOracle));

    let quote =
        MultiSourceOracleAdapter::resolve_price(&env, &client.address, &fallback, xlm, 3600, 500)
            .unwrap();
    assert_eq!(quote.price, 12_345_678);
}
