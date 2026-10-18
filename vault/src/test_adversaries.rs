//! Adversarial cross-contract negative tests for vault (issue #115).
//!
//! Each test exercises one cross-contract trust boundary in the vault with
//! an adversarial counterparty from `test_adversaries`.
//!
//! # Cross-contract boundaries covered
//!
//! 1. Token boundary — vault calls `token.transfer(from, vault, amount)` on
//!    deposit (through `token_validation::safe_inbound_transfer`, which
//!    measures the vault's balance delta) and `token.transfer(vault, to,
//!    amount)` on withdraw.
//!    Adversaries: `FeeOnTransferToken`, `RevertOnTransferToken`,
//!    `ZeroBalanceToken`, `LyingBalanceToken`, `OverflowToken`.
//!
//! 2. Multisig boundary — `withdraw_via_multisig`, `transfer_tag_via_multisig`,
//!    `sweep_untagged_via_multisig`, `transfer_admin_via_multisig`,
//!    `pause_via_multisig`, `unpause_via_multisig`.
//!    Adversaries: `NeverApproveMultisig`, `AlwaysApproveMultisig`.

#![cfg(test)]

extern crate std;

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, IntoVal, Symbol};

use crate::error::Error;
use crate::{Vault, VaultClient};
use test_adversaries::multisig::{AlwaysApproveMultisig, NeverApproveMultisig};
use test_adversaries::tokens::{
    FeeOnTransferToken, LyingBalanceToken, OverflowToken, RevertOnTransferToken, ZeroBalanceToken,
};

// ─── helpers ─────────────────────────────────────────────────────────────────

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

/// Initializes an adversarial token and mints `amount` to `to`.
fn adversary_token(env: &Env, token: Address, to: &Address, amount: i128) -> Address {
    env.invoke_contract::<()>(
        &token,
        &Symbol::new(env, "initialize"),
        soroban_sdk::vec![env, to.into_val(env)],
    );
    env.invoke_contract::<()>(
        &token,
        &Symbol::new(env, "mint"),
        soroban_sdk::vec![env, to.into_val(env), amount.into_val(env)],
    );
    token
}

fn vault_for<'a>(env: &Env, token: &Address) -> VaultClient<'a> {
    let client = VaultClient::new(env, &env.register_contract(None, Vault));
    client.initialize(&Address::generate(env), token);
    client
}

/// Deposits through `token` must fail and leave the ledger untouched.
fn assert_deposit_rejected(env: &Env, token_contract: Address) {
    env.mock_all_auths();
    let depositor = Address::generate(env);
    let token = adversary_token(env, token_contract, &depositor, 1_000_000);
    let client = vault_for(env, &token);
    assert!(client.try_deposit(&depositor, &1, &1_000).is_err());
    assert_eq!(client.balance_of(&1), 0);
    assert_eq!(client.get_total_escrowed(), 0);
}

fn unauthorized() -> Result<soroban_sdk::Error, soroban_sdk::InvokeError> {
    Ok(Error::Unauthorized.into())
}

// ─── Token boundary ──────────────────────────────────────────────────────────

/// A fee-on-transfer token delivers only 90% of the requested amount.
/// The vault measures the balance delta and refuses to credit escrow that
/// never arrived.
#[test]
fn fee_on_transfer_token_deposit_is_rejected_by_delta_check() {
    let env = Env::default();
    env.mock_all_auths();
    let depositor = Address::generate(&env);
    let token = adversary_token(
        &env,
        env.register_contract(None, FeeOnTransferToken),
        &depositor,
        1_000_000,
    );
    let client = vault_for(&env, &token);
    assert_eq!(
        client.try_deposit(&depositor, &1, &1_000_000),
        Err(Ok(Error::DeltaMismatch.into()))
    );
    assert_eq!(client.balance_of(&1), 0);
}

/// A token that always reverts on `transfer` must cause `deposit` to fail
/// with no escrow credited.
#[test]
fn revert_on_transfer_token_causes_deposit_to_fail_with_no_state_change() {
    let env = Env::default();
    assert_deposit_rejected(&env, env.register_contract(None, RevertOnTransferToken));
}

/// A token whose `balance()` always returns 0: the measured delta is 0.
#[test]
fn zero_balance_token_deposit_is_rejected_by_delta_check() {
    let env = Env::default();
    assert_deposit_rejected(&env, env.register_contract(None, ZeroBalanceToken));
}

/// A token whose `balance()` always returns `i128::MAX`: the delta is
/// `MAX - MAX = 0`, which fails the `received < amount` guard.
#[test]
fn lying_balance_token_delta_is_zero_and_deposit_is_rejected() {
    let env = Env::default();
    assert_deposit_rejected(&env, env.register_contract(None, LyingBalanceToken));
}

/// An overflow token (`balance()` = `i128::MAX`) must not let the vault
/// credit any escrow.
#[test]
fn overflow_token_is_caught_by_delta_check() {
    let env = Env::default();
    assert_deposit_rejected(&env, env.register_contract(None, OverflowToken));
}

// ─── Multisig boundary ───────────────────────────────────────────────────────

fn vault_trusting<'a>(env: &Env, multisig: &Address) -> VaultClient<'a> {
    env.mock_all_auths();
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(env))
        .address();
    let client = vault_for(env, &token);
    client.set_multisig(multisig);
    client
}

#[test]
fn vault_pause_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, NeverApproveMultisig);
    let client = vault_trusting(&env, &ms);
    assert_eq!(
        client.try_pause_via_multisig(&ms, &aid(&env, 1)),
        Err(unauthorized())
    );
    assert!(!client.is_paused());
}

#[test]
fn vault_withdraw_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, NeverApproveMultisig);
    let client = vault_trusting(&env, &ms);
    assert_eq!(
        client.try_withdraw_via_multisig(&ms, &aid(&env, 1), &1, &Address::generate(&env), &1),
        Err(unauthorized())
    );
}

#[test]
fn vault_sweep_untagged_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, NeverApproveMultisig);
    let client = vault_trusting(&env, &ms);
    assert_eq!(
        client.try_sweep_untagged_via_multisig(&ms, &aid(&env, 1), &Address::generate(&env)),
        Err(unauthorized())
    );
}

#[test]
fn vault_transfer_admin_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    let ms = env.register_contract(None, NeverApproveMultisig);
    let client = vault_trusting(&env, &ms);
    assert_eq!(
        client.try_transfer_admin_via_multisig(&ms, &aid(&env, 1), &Address::generate(&env)),
        Err(unauthorized())
    );
    assert_eq!(client.get_pending_admin(), None);
}

/// An always-approve multisig at an address that was NOT configured as
/// the vault's trusted multisig must NOT be accepted.
#[test]
fn vault_always_approve_multisig_at_wrong_address_is_rejected() {
    let env = Env::default();
    let trusted = env.register_contract(None, NeverApproveMultisig);
    let client = vault_trusting(&env, &trusted);
    let rogue = env.register_contract(None, AlwaysApproveMultisig);
    assert_eq!(
        client.try_pause_via_multisig(&rogue, &aid(&env, 1)),
        Err(unauthorized())
    );
    assert_eq!(
        client.try_withdraw_via_multisig(&rogue, &aid(&env, 1), &1, &Address::generate(&env), &1),
        Err(unauthorized())
    );
    assert!(!client.is_paused());
}

// ─── Token boundary — safe_inbound_transfer unit tests ───────────────────────

use crate::token_validation::{TokenError, TokenTransferGuard};

/// The real `safe_inbound_transfer` helper must reject a zero-amount request
/// before calling transfer.
#[test]
fn safe_inbound_transfer_rejects_zero_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let token = adversary_token(
        &env,
        env.register_contract(None, FeeOnTransferToken),
        &admin,
        0,
    );

    let result = TokenTransferGuard::safe_inbound_transfer(
        &env,
        &token,
        &admin,
        &Address::generate(&env),
        0, // zero amount — should fail immediately
    );
    assert_eq!(result, Err(TokenError::ZeroTransfer));
}

/// `safe_inbound_transfer` with a fee-on-transfer token must return
/// `DeltaMismatch` because the delta (90%) is less than the requested amount.
#[test]
fn safe_inbound_transfer_with_fee_on_transfer_token_returns_delta_mismatch() {
    let env = Env::default();
    env.mock_all_auths();
    let sender = Address::generate(&env);
    let token = adversary_token(
        &env,
        env.register_contract(None, FeeOnTransferToken),
        &sender,
        1_000_000,
    );

    // Ask for 1_000_000 but only 900_000 will arrive (10% fee).
    let result = TokenTransferGuard::safe_inbound_transfer(
        &env,
        &token,
        &sender,
        &Address::generate(&env),
        1_000_000,
    );
    assert_eq!(result, Err(TokenError::DeltaMismatch));
}

/// `safe_inbound_transfer` with a token that reverts on transfer must
/// propagate the panic rather than returning Ok(0).
#[test]
#[should_panic]
fn safe_inbound_transfer_with_revert_token_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let sender = Address::generate(&env);
    let token = adversary_token(
        &env,
        env.register_contract(None, RevertOnTransferToken),
        &sender,
        1_000_000,
    );

    let _ = TokenTransferGuard::safe_inbound_transfer(
        &env,
        &token,
        &sender,
        &Address::generate(&env),
        1_000_000,
    );
}
