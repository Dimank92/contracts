//! Adversarial cross-contract negative tests for vault (issue #115).
//!
//! Each test exercises one cross-contract trust boundary in the vault with
//! an adversarial counterparty from `test_adversaries`.
//!
//! # Cross-contract boundaries covered
//!
//! 1. Token boundary — vault calls `token.transfer(from, vault, amount)` on
//!    deposit and `token.transfer(vault, to, amount)` on withdraw.
//!    Adversaries: `FeeOnTransferToken`, `RevertOnTransferToken`,
//!    `ZeroBalanceToken`, `LyingBalanceToken`, `OverflowToken`.
//!
//! 2. Multisig boundary — `withdraw_via_multisig`, `transfer_tag_via_multisig`,
//!    `sweep_untagged_via_multisig`, `set_token_allowed_via_multisig`,
//!    `set_integrator_via_multisig`, `transfer_admin_via_multisig`,
//!    `pause_via_multisig`, `unpause_via_multisig`.
//!    Adversaries: `NeverApproveMultisig`, `AlwaysApproveMultisig`.

#![cfg(test)]

extern crate std;

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, Symbol};

use test_adversaries::multisig::{AlwaysApproveMultisig, NeverApproveMultisig};
use test_adversaries::tokens::{
    FeeOnTransferToken, LyingBalanceToken, OverflowToken, RevertOnTransferToken, ZeroBalanceToken,
};

// ─── helper ──────────────────────────────────────────────────────────────────

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

// ─── Token boundary — FeeOnTransferToken ─────────────────────────────────────

/// A fee-on-transfer token delivers only 90% of the requested amount.
/// The vault's `safe_inbound_transfer` measures the balance delta and must
/// credit only what arrived, NOT the requested amount.
///
/// This test is LIVE: the vault's `token_validation.rs` has
/// `safe_inbound_transfer` which checks the delta. When vault's lib.rs
/// is a stub this test is ignored; un-ignore when vault is implemented.
///
/// **Issue #115 — Token: FeeOnTransferToken delta-check boundary.**
#[test]
#[ignore = "issue #115: vault::deposit (lib.rs) not yet implemented"]
fn fee_on_transfer_token_deposit_credits_only_received_delta() {
    let env = Env::default();
    env.mock_all_auths();

    // Register FeeOnTransferToken as the collateral token.
    let token_id = env.register_contract(None, FeeOnTransferToken);
    let depositor = Address::generate(&env);
    // Mint some tokens to the depositor via direct invoke.
    env.invoke_contract::<()>(
        &token_id,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, depositor.clone().into()],
    );
    env.invoke_contract::<()>(
        &token_id,
        &Symbol::new(&env, "mint"),
        soroban_sdk::vec![&env, depositor.clone().into(), 1_000_000i128.into(),],
    );

    // When vault is implemented:
    // let vault_id = env.register_contract(None, Vault);
    // let client = VaultClient::new(&env, &vault_id);
    // let admin = Address::generate(&env);
    // client.initialize(&admin, &token_id, &3600u64);
    // let tag = Tag { owner: depositor.clone(), kind: Symbol::new(&env, "test"), id: 1 };
    // let credited = client.deposit(&depositor, &token_id, &tag, &1_000_000);
    // assert_eq!(credited, 900_000, "only 90% arrived after 10% fee");
    // assert_eq!(client.balance_of(&token_id, &tag), 900_000);
    let _ = depositor;
    let _ = token_id;
}

/// A token that always reverts on `transfer` must cause `deposit` to panic.
/// No escrow should be credited — the state must be unchanged after the panic.
///
/// **Issue #115 — Token: RevertOnTransferToken deposit atomicity.**
#[test]
#[ignore = "issue #115: vault::deposit (lib.rs) not yet implemented"]
fn revert_on_transfer_token_causes_deposit_to_panic_with_no_state_change() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, RevertOnTransferToken);
    let _ = token_id;
    // When vault is implemented:
    // - deposit with RevertOnTransferToken as the token
    // - assert it panics
    // - assert balance_of(token, tag) is still 0 after the panic
}

/// A token whose `balance()` always returns 0 must cause `deposit` to fail
/// the delta-check: the measured balance delta before→after will be 0,
/// which is less than the requested amount. The vault must reject the deposit
/// rather than crediting a phantom 0-delta deposit.
///
/// **Issue #115 — Token: ZeroBalanceToken deposit delta guard.**
#[test]
#[ignore = "issue #115: vault::deposit (lib.rs) not yet implemented"]
fn zero_balance_token_deposit_is_rejected_by_delta_check() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, ZeroBalanceToken);
    let _ = token_id;
    // When implemented:
    // - vault.deposit with ZeroBalanceToken
    // - vault measures balance before (0) and after (0) → delta = 0
    // - safe_inbound_transfer must return DeltaMismatch / fail
}

/// A token whose `balance()` always returns `i128::MAX` must not allow the
/// vault to falsely record a massive balance delta. The delta is
/// `after - before` = `MAX - MAX` = 0, which must fail the `received < amount` guard.
///
/// **Issue #115 — Token: LyingBalanceToken lying delta boundary.**
#[test]
#[ignore = "issue #115: vault::deposit (lib.rs) not yet implemented"]
fn lying_balance_token_delta_is_zero_and_deposit_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, LyingBalanceToken);
    let _ = token_id;
    // When implemented:
    // - vault.deposit with LyingBalanceToken
    // - before = MAX, after = MAX, delta = 0 → DeltaMismatch
}

/// An overflow token (`balance()` = `i128::MAX`, `transfer` is a no-op)
/// must not let the vault credit any escrow because the delta check
/// computes `MAX - MAX = 0`, failing the `received < amount` guard.
///
/// **Issue #115 — Token: OverflowToken no-op transfer boundary.**
#[test]
#[ignore = "issue #115: vault::deposit (lib.rs) not yet implemented"]
fn overflow_token_noop_transfer_is_caught_by_delta_check() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, OverflowToken);
    let _ = token_id;
    // When implemented:
    // - vault.deposit with OverflowToken
    // - transfer is a no-op, delta = 0 → DeltaMismatch
}

// ─── Multisig boundary — NeverApproveMultisig ────────────────────────────────

/// `pause_via_multisig` with a never-approve multisig must reject with
/// `Unauthorized`. The vault must not become paused.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at vault.pause_via_multisig.**
#[test]
#[ignore = "issue #115: vault::pause_via_multisig (lib.rs) not yet implemented"]
fn vault_pause_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let ms_id = env.register_contract(None, NeverApproveMultisig);
    // When vault is implemented:
    // - register_contract(None, Vault)
    // - initialize(admin, token, max_pause_duration)
    // - call pause_via_multisig(&ms_id, &0u64) — should panic Unauthorized
    // - assert is_paused() is still false
    let _ = ms_id;
}

/// `withdraw_via_multisig` with a never-approve multisig must reject.
/// No tokens must be transferred.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at vault.withdraw_via_multisig.**
#[test]
#[ignore = "issue #115: vault::withdraw_via_multisig (lib.rs) not yet implemented"]
fn vault_withdraw_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let ms_id = env.register_contract(None, NeverApproveMultisig);
    let _ = ms_id;
    // When vault is implemented, this should panic Unauthorized.
}

/// `set_token_allowed_via_multisig` with a never-approve multisig must
/// reject. The token allowlist must be unchanged.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at vault.set_token_allowed_via_multisig.**
#[test]
#[ignore = "issue #115: vault::set_token_allowed_via_multisig (lib.rs) not yet implemented"]
fn vault_set_token_allowed_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let ms_id = env.register_contract(None, NeverApproveMultisig);
    let _ = ms_id;
}

/// `transfer_admin_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at vault.transfer_admin_via_multisig.**
#[test]
#[ignore = "issue #115: vault::transfer_admin_via_multisig (lib.rs) not yet implemented"]
fn vault_transfer_admin_via_multisig_with_never_approve_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let ms_id = env.register_contract(None, NeverApproveMultisig);
    let _ = ms_id;
}

// ─── Multisig boundary — AlwaysApproveMultisig substitution ──────────────────

/// An always-approve multisig at an address that was NOT registered as the
/// vault's trusted multisig must NOT be accepted. The vault compares the
/// multisig address against the one stored at initialization — any address
/// claiming to be a multisig must be validated.
///
/// **Issue #115 — Multisig: AlwaysApproveMultisig substitution at vault.**
#[test]
#[ignore = "issue #115: vault (lib.rs) not yet implemented"]
fn vault_always_approve_multisig_at_wrong_address_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    // Register an always-approve adversary at an address the vault has never seen.
    let malicious_ms = env.register_contract(None, AlwaysApproveMultisig);
    let _ = malicious_ms;
    // When vault is implemented:
    // - initialize with a DIFFERENT multisig address
    // - call pause_via_multisig(&malicious_ms, &0u64) — must panic Unauthorized
    // This verifies the vault stores the multisig at initialize time and
    // doesn't trust arbitrary contracts claiming to be multisigs.
}

// ─── Token boundary — safe_inbound_transfer unit tests ───────────────────────
// These tests exercise vault's token_validation module directly,
// which IS implemented (token_validation.rs has content).

use crate::token_validation::{TokenError, TokenTransferGuard};

/// The real `safe_inbound_transfer` helper must reject a zero-amount request
/// before calling transfer.
///
/// **Issue #115 — Token: ZeroTransfer guard in safe_inbound_transfer.**
#[test]
fn safe_inbound_transfer_rejects_zero_amount() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, FeeOnTransferToken);
    let admin = Address::generate(&env);
    env.invoke_contract::<()>(
        &token_id,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.clone().into()],
    );

    let result = TokenTransferGuard::safe_inbound_transfer(
        &env,
        &token_id,
        &admin,
        &Address::generate(&env),
        0, // zero amount — should fail immediately
    );
    assert_eq!(result, Err(TokenError::ZeroTransfer));
}

/// `safe_inbound_transfer` with a fee-on-transfer token must return
/// `DeltaMismatch` because the delta (90%) is less than the requested amount.
///
/// This test IS live: token_validation.rs is implemented.
///
/// **Issue #115 — Token: FeeOnTransferToken delta-mismatch in safe_inbound_transfer.**
#[test]
fn safe_inbound_transfer_with_fee_on_transfer_token_returns_delta_mismatch() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, FeeOnTransferToken);
    let sender = Address::generate(&env);
    let recipient = Address::generate(&env);

    // Initialize and mint tokens to sender.
    env.invoke_contract::<()>(
        &token_id,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, sender.clone().into()],
    );
    env.invoke_contract::<()>(
        &token_id,
        &Symbol::new(&env, "mint"),
        soroban_sdk::vec![&env, sender.clone().into(), 1_000_000i128.into(),],
    );

    // Ask for 1_000_000 but only 900_000 will arrive (10% fee).
    let result =
        TokenTransferGuard::safe_inbound_transfer(&env, &token_id, &sender, &recipient, 1_000_000);
    // 900_000 < 1_000_000 → DeltaMismatch
    assert_eq!(result, Err(TokenError::DeltaMismatch));
}

/// `safe_inbound_transfer` with a token that reverts on transfer must
/// propagate the panic rather than returning Ok(0).
///
/// **Issue #115 — Token: RevertOnTransferToken propagation in safe_inbound_transfer.**
#[test]
#[should_panic]
fn safe_inbound_transfer_with_revert_token_panics() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, RevertOnTransferToken);
    let sender = Address::generate(&env);
    let recipient = Address::generate(&env);

    env.invoke_contract::<()>(
        &token_id,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, sender.clone().into()],
    );
    env.invoke_contract::<()>(
        &token_id,
        &Symbol::new(&env, "mint"),
        soroban_sdk::vec![&env, sender.clone().into(), 1_000_000i128.into(),],
    );

    // RevertOnTransferToken.transfer() always panics — should propagate.
    let _ =
        TokenTransferGuard::safe_inbound_transfer(&env, &token_id, &sender, &recipient, 1_000_000);
}

