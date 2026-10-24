//! Adversarial suite for the `options_market` contract.
//!
//! These tests exercise settlement and collateral abuse paths through the
//! contract's public entrypoints only. No back doors into storage are used.
//!
//! Scenarios covered:
//! - Double-claim of a settled position (replay of `claim`).
//! - Under-collateralized settlement: a writer attempting to settle a position
//!   whose collateral does not cover the payout.
//! - Claim before settlement / claim by a non-holder.
//!
//! All tests are deterministic: no wall-clock or RNG dependence. Time is
//! advanced only through the ledger's deterministic timestamp helpers.

use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_sdk::{Address, Env};

use options_market::{OptionsMarket, OptionsMarketClient};

/// Deterministic base timestamp used across the suite.
const BASE_TS: u64 = 1_700_000_000;

/// Build a fresh environment with a deterministic ledger timestamp.
fn setup_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|li| {
        li.timestamp = BASE_TS;
    });
    env
}

/// Deploy and initialize the options_market contract, returning a client.
fn deploy(env: &Env, admin: &Address) -> OptionsMarketClient<'_> {
    let contract_id = env.register_contract(None, OptionsMarket);
    let client = OptionsMarketClient::new(env, &contract_id);
    client.initialize(admin);
    client
}

/// A settled position cannot be claimed twice: the second claim must fail.
#[test]
fn double_claim_is_rejected() {
    let env = setup_env();
    let admin = Address::generate(&env);
    let writer = Address::generate(&env);
    let buyer = Address::generate(&env);

    let client = deploy(&env, &admin);

    // Writer opens a fully-collateralized position.
    let strike: i128 = 100;
    let collateral: i128 = 1_000;
    let position_id = client.write_position(&writer, &strike, &collateral);

    // Buyer purchases the option.
    client.buy_option(&buyer, &position_id);

    // Advance past expiry and settle at a payout the collateral covers.
    env.ledger().with_mut(|li| {
        li.timestamp = BASE_TS + 86_400;
    });
    let settlement_price: i128 = 120;
    client.settle(&admin, &position_id, &settlement_price);

    // First claim by the holder succeeds.
    client.claim(&buyer, &position_id);

    // Second claim by the same holder must be rejected (already claimed).
    let second = client.try_claim(&buyer, &position_id);
    assert!(
        second.is_err(),
        "double-claim must be rejected after the position is settled and claimed"
    );
}

/// A writer cannot claim the buyer's payout after settlement.
#[test]
fn writer_cannot_claim_buyer_payout() {
    let env = setup_env();
    let admin = Address::generate(&env);
    let writer = Address::generate(&env);
    let buyer = Address::generate(&env);

    let client = deploy(&env, &admin);

    let strike: i128 = 100;
    let collateral: i128 = 1_000;
    let position_id = client.write_position(&writer, &strike, &collateral);
    client.buy_option(&buyer, &position_id);

    env.ledger().with_mut(|li| {
        li.timestamp = BASE_TS + 86_400;
    });
    client.settle(&admin, &position_id, &120);

    // The writer is not the holder and must not be able to claim the payout.
    let writer_claim = client.try_claim(&writer, &position_id);
    assert!(
        writer_claim.is_err(),
        "writer must not be able to claim the buyer's payout"
    );
}

/// Claiming before settlement must be rejected.
#[test]
fn claim_before_settlement_is_rejected() {
    let env = setup_env();
    let admin = Address::generate(&env);
    let writer = Address::generate(&env);
    let buyer = Address::generate(&env);

    let client = deploy(&env, &admin);

    let position_id = client.write_position(&writer, &100, &1_000);
    client.buy_option(&buyer, &position_id);

    // No settlement has occurred yet.
    let early = client.try_claim(&buyer, &position_id);
    assert!(
        early.is_err(),
        "claim before settlement must be rejected"
    );
}

/// An under-collateralized position cannot be settled for a payout that
/// exceeds the collateral the writer posted.
#[test]
fn under_collateralized_settlement_is_rejected() {
    let env = setup_env();
    let admin = Address::generate(&env);
    let writer = Address::generate(&env);
    let buyer = Address::generate(&env);

    let client = deploy(&env, &admin);

    // Writer posts collateral that is far below the potential payout.
    let strike: i128 = 100;
    let collateral: i128 = 10;
    let position_id = client.write_position(&writer, &strike, &collateral);
    client.buy_option(&buyer, &position_id);

    env.ledger().with_mut(|li| {
        li.timestamp = BASE_TS + 86_400;
    });

    // Settlement price implies a payout far exceeding the posted collateral.
    let settlement_price: i128 = 10_000;
    let settle_result = client.try_settle(&admin, &position_id, &settlement_price);
    assert!(
        settle_result.is_err(),
        "settlement must be rejected when payout exceeds posted collateral"
    );
}

/// Only the admin may settle a position; an arbitrary address must not be able
/// to force settlement at an attacker-chosen price.
#[test]
fn non_admin_cannot_settle() {
    let env = setup_env();
    let admin = Address::generate(&env);
    let writer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let attacker = Address::generate(&env);

    let client = deploy(&env, &admin);

    let position_id = client.write_position(&writer, &100, &1_000);
    client.buy_option(&buyer, &position_id);

    env.ledger().with_mut(|li| {
        li.timestamp = BASE_TS + 86_400;
    });

    let attacker_settle = client.try_settle(&attacker, &position_id, &120);
    assert!(
        attacker_settle.is_err(),
        "non-admin must not be able to settle a position"
    );
}

/// A position cannot be settled twice; a second settlement must be rejected.
#[test]
fn double_settlement_is_rejected() {
    let env = setup_env();
    let admin = Address::generate(&env);
    let writer = Address::generate(&env);
    let buyer = Address::generate(&env);

    let client = deploy(&env, &admin);

    let position_id = client.write_position(&writer, &100, &1_000);
    client.buy_option(&buyer, &position_id);

    env.ledger().with_mut(|li| {
        li.timestamp = BASE_TS + 86_400;
    });

    client.settle(&admin, &position_id, &120);

    // A second settlement attempt must fail.
    let second = client.try_settle(&admin, &position_id, &130);
    assert!(
        second.is_err(),
        "double settlement must be rejected"
    );
}