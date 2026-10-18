//! Adversarial cross-contract negative tests for options_market (issue #115).
//!
//! Each test exercises one cross-contract trust boundary with an adversarial
//! counterparty from `test_adversaries`. Tests that currently expose a
//! vulnerability, or need an adversary that doesn't exist yet, are marked
//! `#[ignore]` with the reason so they show up in the suite (not silently
//! skipped) and are un-ignored by the PRs that close them.
//!
//! # Cross-contract boundaries covered
//!
//! 1. Oracle boundary — `set_settlement_price_from_oracle` calls `oracle.get_price(symbol)`
//!    on the market's configured oracle.
//!    Adversaries: `AlwaysNoneOracle`, `RevertingOracle`, `NegativePriceOracle`,
//!    `ExtremeHighOracle`.
//!
//! 2. Vault boundary — `escrow_series_to_vault` calls `vault.deposit(...)`;
//!    `claim_refund_from_vault` calls `vault.withdraw(...)`.
//!    Adversaries: `NoOpVault`, `RevertOnWithdrawVault`.
//!
//! 3. Multisig boundary — every `_via_multisig` entrypoint goes through
//!    `zenith_common::require_admin_or_multisig` (trusted multisig only,
//!    `is_approved`, one call per approval).
//!    Adversaries: `NeverApproveMultisig`, `AlwaysApproveMultisig`.
//!
//! 4. Token boundary — collateral and premium transfers via `soroban_sdk::token::Client`.
//!    Adversaries: `FeeOnTransferToken`, `RevertOnTransferToken`, `LyingBalanceToken`.

#![cfg(test)]

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, BytesN, Env, IntoVal, Symbol,
};

use crate::error::Error;
use crate::{OptionType, OptionsMarket, OptionsMarketClient, SeriesState};
use test_adversaries::multisig::{AlwaysApproveMultisig, NeverApproveMultisig};
use test_adversaries::oracle::{
    AlwaysNoneOracle, ExtremeHighOracle, NegativePriceOracle, RevertingOracle,
};
use test_adversaries::tokens::RevertOnTransferToken;

const UNIT: i128 = 10_000_000;

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

fn market<'a>(env: &Env, token: &Address) -> OptionsMarketClient<'a> {
    env.mock_all_auths();
    let client = OptionsMarketClient::new(env, &env.register_contract(None, OptionsMarket));
    client.initialize(
        &Address::generate(env),
        &Address::generate(env),
        token,
        &Address::generate(env),
    );
    client
}

fn usdc(env: &Env) -> Address {
    env.register_stellar_asset_contract_v2(Address::generate(env))
        .address()
}

/// A market whose settlement oracle is `oracle`, with one call series
/// whose expiry has just passed. Returns `(client, series_id)`.
fn expired_series_with_oracle<'a>(env: &Env, oracle: Address) -> (OptionsMarketClient<'a>, u64) {
    let client = market(env, &usdc(env));
    env.invoke_contract::<()>(
        &oracle,
        &Symbol::new(env, "initialize"),
        soroban_sdk::vec![env, Address::generate(env).into_val(env)],
    );
    client.set_oracle(&oracle);
    let expiry = env.ledger().timestamp() + 30 * 86_400;
    let series_id = client.create_series(
        &Symbol::new(env, "XLM"),
        &OptionType::Call,
        &(70 * UNIT),
        &expiry,
        &(4 * UNIT),
        &450_000_000,
    );
    env.ledger().with_mut(|l| l.timestamp = expiry);
    (client, series_id)
}

fn assert_unsettled(client: &OptionsMarketClient, series_id: u64) {
    let series = client.get_series(&series_id).unwrap();
    assert_eq!(series.settlement_price, None);
    assert_eq!(series.state, SeriesState::Active);
}

// ─── Oracle boundary ─────────────────────────────────────────────────────────

/// `set_settlement_price_from_oracle` must NOT silently settle at price 0
/// when the oracle returns `None`.
#[test]
fn oracle_none_price_must_not_settle_the_series() {
    let env = Env::default();
    let oracle = env.register_contract(None, AlwaysNoneOracle);
    let (client, series_id) = expired_series_with_oracle(&env, oracle.clone());
    assert_eq!(
        client.try_set_settlement_price_from_oracle(&series_id, &oracle),
        Err(Ok(Error::PriceNotSet.into()))
    );
    assert_unsettled(&client, series_id);
}

/// A reverting oracle contract must cause the whole call to revert, NOT
/// partially update the series state.
#[test]
fn oracle_revert_must_propagate_and_not_partially_settle() {
    let env = Env::default();
    let oracle = env.register_contract(None, RevertingOracle);
    let (client, series_id) = expired_series_with_oracle(&env, oracle.clone());
    assert!(client
        .try_set_settlement_price_from_oracle(&series_id, &oracle)
        .is_err());
    assert_unsettled(&client, series_id);
}

/// An oracle returning a negative price must be rejected before any
/// storage is updated.
#[test]
fn oracle_negative_price_must_be_rejected_not_stored() {
    let env = Env::default();
    let oracle = env.register_contract(None, NegativePriceOracle);
    let (client, series_id) = expired_series_with_oracle(&env, oracle.clone());
    assert_eq!(
        client.try_set_settlement_price_from_oracle(&series_id, &oracle),
        Err(Ok(Error::PriceNotSet.into()))
    );
    assert_unsettled(&client, series_id);
}

/// An oracle returning `i128::MAX` must not make payout math wrap: the
/// series may settle at that price, but exercising must fail loudly
/// instead of paying a wrapped amount.
#[test]
fn oracle_extreme_high_price_must_not_overflow_payout_math() {
    let env = Env::default();
    let token = usdc(&env);
    let client = market(&env, &token);
    let oracle = env.register_contract(None, ExtremeHighOracle);
    env.invoke_contract::<()>(
        &oracle,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, Address::generate(&env).into_val(&env)],
    );
    client.set_oracle(&oracle);
    let expiry = env.ledger().timestamp() + 30 * 86_400;
    let series_id = client.create_series(
        &Symbol::new(&env, "XLM"),
        &OptionType::Call,
        &(70 * UNIT),
        &expiry,
        &(4 * UNIT),
        &450_000_000,
    );
    let buyer = Address::generate(&env);
    soroban_sdk::token::StellarAssetClient::new(&env, &token).mint(&buyer, &(1_000 * UNIT));
    let pos_id = client.buy_option(&buyer, &series_id, &(2 * UNIT), &(50 * UNIT));

    env.ledger().with_mut(|l| l.timestamp = expiry);
    client.set_settlement_price_from_oracle(&series_id, &oracle);
    assert_eq!(
        client.get_series(&series_id).unwrap().settlement_price,
        Some(i128::MAX)
    );

    let before = soroban_sdk::token::Client::new(&env, &token).balance(&buyer);
    assert!(client.try_exercise(&buyer, &pos_id).is_err());
    assert_eq!(
        soroban_sdk::token::Client::new(&env, &token).balance(&buyer),
        before
    );
    assert!(!client.get_position(&pos_id).unwrap().is_exercised);
}

/// The oracle used for permissionless settlement must be the market's
/// configured one — not whichever contract the caller names.
#[test]
fn an_unconfigured_oracle_cannot_settle_a_series() {
    let env = Env::default();
    let trusted = env.register_contract(None, AlwaysNoneOracle);
    let (client, series_id) = expired_series_with_oracle(&env, trusted);
    let rogue = env.register_contract(None, ExtremeHighOracle);
    env.invoke_contract::<()>(
        &rogue,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, Address::generate(&env).into_val(&env)],
    );
    assert_eq!(
        client.try_set_settlement_price_from_oracle(&series_id, &rogue),
        Err(Ok(Error::Unauthorized.into()))
    );
    assert_unsettled(&client, series_id);
}

// ─── Vault boundary ──────────────────────────────────────────────────────────

/// `escrow_series_to_vault` calls `vault.deposit`; a no-op vault must cause
/// the escrow operation to fail rather than silently zeroing the
/// liability. (The market already checks the vault's tag balance grew by
/// the escrowed amount after the deposit.)
#[test]
#[ignore = "issue #115: test_adversaries::vault mirrors the multi-token Tag interface, not the single-token u64-tag vault options_market calls"]
fn noop_vault_deposit_must_cause_escrow_to_fail() {}

/// `claim_refund_from_vault` calls `vault.withdraw`; a vault that always
/// reverts on withdraw must propagate the revert rather than marking the
/// position as refunded.
#[test]
#[ignore = "issue #115: test_adversaries::vault mirrors the multi-token Tag interface, not the single-token u64-tag vault options_market calls"]
fn vault_revert_on_withdraw_must_not_mark_position_refunded() {}

// ─── Multisig boundary ───────────────────────────────────────────────────────

/// A multisig that never approves must cause every `_via_multisig`
/// entrypoint to reject with `Unauthorized`.
#[test]
fn never_approve_multisig_rejects_pause_via_multisig() {
    let env = Env::default();
    let client = market(&env, &usdc(&env));
    let multisig = env.register_contract(None, NeverApproveMultisig);
    client.set_multisig(&multisig);

    assert_eq!(
        client.try_pause_via_multisig(&multisig, &aid(&env, 1)),
        Err(Ok(Error::Unauthorized.into()))
    );
    assert_eq!(
        client.try_set_fee_rate_via_multisig(&multisig, &aid(&env, 1), &10),
        Err(Ok(Error::Unauthorized.into()))
    );
    assert!(!client.is_paused());
}

/// An always-approve multisig at an address that isn't the market's
/// trusted multisig must NOT be accepted.
#[test]
fn always_approve_multisig_at_wrong_address_must_be_rejected() {
    let env = Env::default();
    let client = market(&env, &usdc(&env));
    let trusted = env.register_contract(None, NeverApproveMultisig);
    client.set_multisig(&trusted);
    let rogue = env.register_contract(None, AlwaysApproveMultisig);
    let attacker = Address::generate(&env);

    assert_eq!(
        client.try_transfer_admin_via_multisig(&rogue, &aid(&env, 1), &attacker),
        Err(Ok(Error::Unauthorized.into()))
    );
    assert_eq!(
        client.try_set_oracle_via_multisig(&rogue, &aid(&env, 2), &attacker),
        Err(Ok(Error::Unauthorized.into()))
    );
    assert_eq!(client.get_pending_admin(), None);
}

// ─── Token boundary ──────────────────────────────────────────────────────────

/// A fee-on-transfer collateral token must not let positions record more
/// than actually arrived.
#[test]
#[ignore = "issue #115: options_market records premiums/collateral at the requested amount; it doesn't measure token deltas yet"]
fn fee_on_transfer_token_must_not_over_credit_escrow() {}

/// A token that always reverts on transfer must cause buy_option to fail
/// atomically — no position, no open interest.
#[test]
fn revert_on_transfer_token_causes_buy_option_to_revert_atomically() {
    let env = Env::default();
    env.mock_all_auths();
    let buyer = Address::generate(&env);
    let token = env.register_contract(None, RevertOnTransferToken);
    env.invoke_contract::<()>(
        &token,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, buyer.into_val(&env)],
    );
    env.invoke_contract::<()>(
        &token,
        &Symbol::new(&env, "mint"),
        soroban_sdk::vec![&env, buyer.into_val(&env), (1_000 * UNIT).into_val(&env)],
    );
    let client = market(&env, &token);
    let series_id = client.create_series(
        &Symbol::new(&env, "XLM"),
        &OptionType::Call,
        &(70 * UNIT),
        &(env.ledger().timestamp() + 30 * 86_400),
        &(4 * UNIT),
        &450_000_000,
    );

    assert!(client
        .try_buy_option(&buyer, &series_id, &UNIT, &(50 * UNIT))
        .is_err());
    assert!(client.get_user_positions(&buyer).is_empty());
    assert_eq!(client.get_series(&series_id).unwrap().open_interest, 0);
    assert_eq!(client.get_stats().1, 0);
}

/// A token whose `balance()` always returns `i128::MAX` must not cause a
/// false positive in any balance-delta check.
#[test]
#[ignore = "issue #115: options_market doesn't measure token deltas yet; the vault-backed path is covered by vault's own lying-token test"]
fn lying_balance_token_must_not_bypass_delta_check() {}
