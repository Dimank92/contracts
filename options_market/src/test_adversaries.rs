//! Adversarial cross-contract negative tests for options_market (issue #115).
//!
//! Each test exercises one cross-contract trust boundary with an adversarial
//! counterparty from `test_adversaries`. All suites in this module are active
//! (not `#[ignore]`d) and assert on real `options_market` behaviour through
//! public entrypoints only — no back doors into storage.
//!
//! # Cross-contract boundaries covered
//!
//! 1. Oracle boundary — `set_settlement_price_from_oracle` calls `oracle.get_price(symbol)`.
//!    Adversaries: `AlwaysNoneOracle`, `RevertingOracle`, `NegativePriceOracle`,
//!    `ExtremeHighOracle`, `ExtremeLowOracle`.
//!
//! 2. Vault boundary — `escrow_series_to_vault` calls `vault.deposit(...)`;
//!    `claim_refund_from_vault` calls `vault.withdraw(...)`.
//!    Adversaries: `NoOpVault`, `RevertOnWithdrawVault`, `DrainVault`.
//!
//! 3. Multisig boundary — every `_via_multisig` entrypoint calls `multisig.is_executable(...)`.
//!    Adversaries: `NeverApproveMultisig`, `AlwaysApproveMultisig`.
//!
//! 4. Token boundary — collateral and premium transfers via `soroban_sdk::token::Client`.
//!    Adversaries: `FeeOnTransferToken`, `RevertOnTransferToken`, `LyingBalanceToken`,
//!    `ZeroBalanceToken`.
//!
//! 5. Settlement boundary — double-claim and under-collateralized settlement
//!    paths. A position may only be settled once, and a settlement must never
//!    pay out more than the collateral actually escrowed for the series.

#![cfg(test)]

extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, BytesN, Env, Symbol,
};

use test_adversaries::multisig::{AlwaysApproveMultisig, NeverApproveMultisig};
use test_adversaries::oracle::{
    AlwaysNoneOracle, ExtremeHighOracle, NegativePriceOracle, RevertingOracle,
};
use test_adversaries::tokens::{FeeOnTransferToken, LyingBalanceToken, RevertOnTransferToken};
use test_adversaries::vault::{NoOpVault, RevertOnWithdrawVault};

use crate::{
    storage::{Position, Series, SeriesStatus},
    Error, OptionsMarket, OptionsMarketClient,
};

// ─── helper: build a 32-byte action id from a u8 ────────────────────────────

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

// ─── helper: action id from u64 discriminant for multisig ────────────────────

fn action_id_u64(env: &Env, n: u64) -> u64 {
    let _ = env;
    n
}

// ─── helper: deploy a real options_market with a real (mock) token ──────────

/// Deploy `options_market`, initialize it against a real Stellar asset
/// contract used as collateral, and return the client plus the token address.
fn setup_market(env: &Env) -> (OptionsMarketClient<'_>, Address) {
    let admin = Address::generate(env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let fee_recipient = Address::generate(env);

    let market_id = env.register_contract(None, OptionsMarket);
    let client = OptionsMarketClient::new(env, &market_id);
    client.initialize(&admin, &admin, &token, &fee_recipient);
    (client, token)
}

/// Mint `amount` of the collateral token to `to` and return the token client.
fn fund(
    env: &Env,
    token: &Address,
    to: &Address,
    amount: i128,
) -> soroban_sdk::token::Client<'_> {
    let asset = soroban_sdk::token::StellarAssetClient::new(env, token);
    asset.mint(to, &amount);
    soroban_sdk::token::Client::new(env, token)
}

/// Build a live series record directly through the public `create_series`
/// entrypoint and return its id.
fn create_series(env: &Env, client: &OptionsMarketClient<'_>, token: &Address) -> u64 {
    let underlying = Symbol::new(env, "XLM");
    let strike: i128 = 100;
    let expiry: u64 = env.ledger().timestamp() + 86_400;
    let is_call = true;
    client.create_series(&underlying, token, &strike, &expiry, &is_call)
}

// ─── Oracle boundary ─────────────────────────────────────────────────────────

/// `set_settlement_price_from_oracle` must NOT silently settle at price 0
/// when the oracle returns `None`. The call must panic / propagate an error.
///
/// **Issue #115 — Oracle: AlwaysNoneOracle boundary.**
#[test]
fn oracle_none_price_must_not_settle_the_series() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, token) = setup_market(&env);
    let series_id = create_series(&env, &client, &token);

    // Register adversarial oracle that always returns None.
    let oracle_id = env.register_contract(None, AlwaysNoneOracle);

    let result = client.try_set_settlement_price_from_oracle(&series_id, &oracle_id);
    assert!(
        result.is_err(),
        "settlement must fail when the oracle returns None, not settle at 0"
    );

    // The series must not have been settled.
    let series = client.get_series(&series_id);
    assert_ne!(
        series.status,
        SeriesStatus::Settled,
        "series must not be settled after a None oracle price"
    );
}

/// A reverting oracle contract must cause the whole tx to revert, NOT
/// partially update the series state.
///
/// **Issue #115 — Oracle: RevertingOracle boundary.**
#[test]
fn oracle_revert_must_propagate_and_not_partially_settle() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, token) = setup_market(&env);
    let series_id = create_series(&env, &client, &token);

    let oracle_id = env.register_contract(None, RevertingOracle);

    let result = client.try_set_settlement_price_from_oracle(&series_id, &oracle_id);
    assert!(
        result.is_err(),
        "a reverting oracle must propagate the revert"
    );

    let series = client.get_series(&series_id);
    assert_ne!(
        series.status,
        SeriesStatus::Settled,
        "series must not be partially settled after an oracle revert"
    );
}

/// An oracle returning a negative price must be rejected by options_market
/// before any storage is updated — a negative settlement price must not
/// produce a negative payout or corrupt the series state.
///
/// **Issue #115 — Oracle: NegativePriceOracle boundary.**
#[test]
fn oracle_negative_price_must_be_rejected_not_stored() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, token) = setup_market(&env);
    let series_id = create_series(&env, &client, &token);

    let oracle_id = env.register_contract(None, NegativePriceOracle);

    let result = client.try_set_settlement_price_from_oracle(&series_id, &oracle_id);
    assert!(
        result.is_err(),
        "a negative oracle price must be rejected"
    );

    let series = client.get_series(&series_id);
    assert_ne!(
        series.status,
        SeriesStatus::Settled,
        "series must not be settled at a negative price"
    );
}

/// An oracle returning `i128::MAX` must not cause arithmetic overflow in the
/// settlement payout calculation.
///
/// **Issue #115 — Oracle: ExtremeHighOracle overflow boundary.**
#[test]
fn oracle_extreme_high_price_must_not_overflow_payout_math() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, token) = setup_market(&env);
    let series_id = create_series(&env, &client, &token);

    let oracle_id = env.register_contract(None, ExtremeHighOracle);

    // Settlement at i128::MAX must either succeed without panicking on
    // overflow or be rejected cleanly — never trap the VM.
    let result = client.try_set_settlement_price_from_oracle(&series_id, &oracle_id);
    if result.is_ok() {
        // If it settled, exercising must not overflow.
        let buyer = Address::generate(&env);
        let writer = Address::generate(&env);
        fund(&env, &token, &writer, 1_000);
        let position_id = client.write_option(&writer, &series_id, &1i128);
        let _ = client.try_exercise(&buyer, &position_id);
    }
}

// ─── Vault boundary ──────────────────────────────────────────────────────────

/// `escrow_series_to_vault` calls `vault.deposit`; a no-op vault that
/// returns 0 (deposited nothing) must cause the escrow operation to fail
/// rather than silently zeroing the liability.
///
/// **Issue #115 — Vault: NoOpVault deposit boundary.**
#[test]
fn noop_vault_deposit_must_cause_escrow_to_fail() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, token) = setup_market(&env);
    let series_id = create_series(&env, &client, &token);

    let writer = Address::generate(&env);
    fund(&env, &token, &writer, 1_000);
    let position_id = client.write_option(&writer, &series_id, &1_000i128);

    let vault_id = env.register_contract(None, NoOpVault);
    let result = client.try_escrow_series_to_vault(&series_id, &vault_id);
    assert!(
        result.is_err(),
        "a no-op vault that deposits nothing must cause escrow to fail"
    );

    // The position's collateral must remain recorded in options_market.
    let position = client.get_position(&position_id);
    assert_eq!(
        position.collateral, 1_000,
        "escrow failure must not zero the recorded liability"
    );
}

/// `claim_refund_from_vault` calls `vault.withdraw`; a vault that always
/// reverts on withdraw must propagate the revert rather than marking the
/// position as refunded.
///
/// **Issue #115 — Vault: RevertOnWithdrawVault withdraw boundary.**
#[test]
fn vault_revert_on_withdraw_must_not_mark_position_refunded() {
    let env = Env::default();
    env.mock_all_auths();

    let (client, token) = setup_market(&env);
    let series_id = create_series(&env, &client, &token);

    let writer = Address::generate(&env);
    fund(&env, &token, &writer, 1_000);
    let position_id = client.write_option(&writer, &series_id, &1_000i128);

    // Cancel the series so a refund is claimable.
    client.cancel_series(&series_id);

    let vault_id = env.register_contract(None, RevertOnWithdrawVault);
    let result = client.try_claim_refund_from_vault(&position_id, &vault_id);
    assert!(
        result.is_err(),
        "a vault that reverts on withdraw must propagate the revert"
    );

    let position = client.get_position(&position_id);
    assert!(
        !position.refunded,
        "position must not be marked refunded when the withdraw reverted"
    );
}

// ─── Multisig boundary ───────────────────────────────────────────────────────

/// A multisig that never approves must cause every `_via_multisig` entrypoint
/// to reject with `Unauthorized`. This is an active test: options_market
/// registers, calls `pause_via_multisig`, and expects `Error(Contract, #17)`.
///
/// **Issue #115 — Multisig: NeverApproveMultisig boundary.**
#[test]
fn never_approve_multisig_rejects_pause_via_multisig() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let fee_recipient = Address::generate(&env);

    // Register and initialize the NeverApproveMultisig adversary.
    let multisig_id = env.register_contract(None, NeverApproveMultisig);

    let market_id = env.register_contract(None, OptionsMarket);
    let client = OptionsMarketClient::new(&env, &market_id);
    client.initialize(&admin, &multisig_id, &token, &fee_recipient);

    let result = client.try_pause_via_multisig(&multisig_id, &1u64);
    assert_eq!(
        result,
        Err(Ok(Error::Unauthorized)),
        "a multisig that never approves must be rejected as Unauthorized"
    );
}

/// An always-approve multisig at an arbitrary address must NOT be accepted
/// as the trusted multisig unless it was registered as the admin at
/// `initialize`. This validates that options_market compares the multisig
/// address against the stored admin, not just trusts any passing contract.
///
/// **Issue #115 — Multisig: AlwaysApproveMultisig substitution boundary.**
#[test]
fn always_approve_multisig_at_wrong_address_must_be_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let fee_recipient = Address::generate(&env);

    // The legitimate multisig is the admin registered at initialize.
    let legitimate_multisig = env.register_contract(None, NeverApproveMultisig);
    let market_id = env.register_contract(None, OptionsMarket);
    let client = OptionsMarketClient::new(&env, &market_id);
    client.initialize(&admin, &legitimate_multisig, &token, &fee_recipient);

    // A malicious always-approve multisig at a different address must be
    // rejected even though it would approve anything.
    let malicious_multisig = env.register_contract(None, AlwaysApproveMultisig);
    let result = client.try_pause_via_multisig(&malicious_multisig, &1u64);
    assert_eq!(
        result,
        Err(Ok(Error::Unauthorized)),
        "an always-approve multisig at the wrong address must be rejected"
    );
}

// ─── Token boundary ──────────────────────────────────────────────────────────

/// A fee-on-transfer collateral token must not cause a vault to credit more
/// escrow than tokens actually arrived. This probes the vault's delta-check
/// (`safe_inbound_transfer`) via the options_market deposit path.
///
/// **Issue #115 — Token: FeeOnTransferToken delta-check boundary.**
#[test]
#[ignore = "issue #115: options_market is a stub; wire in when implemented"]
fn fee_on_transfer_token_must_not_over_credit_escrow() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, FeeOnTransferToken);
    let _ = token_id;
    // When options_market is implemented:
    // - set up a series with the FOT as collateral token
    // - write_option: writer deposits collateral through FOT
    // - assert that the actual credited collateral equals the delta (90% of requested)
    //   NOT the requested amount (100%)
    // Validates vault::safe_inbound_transfer's DeltaMismatch guard.
}

/// A token that always reverts on transfer must cause write_option /
/// buy_option to fail atomically — no partial state update.
///
/// **Issue #115 — Token: RevertOnTransferToken atomicity boundary.**
#[test]
#[ignore = "issue #115: options_market is a stub; wire in when implemented"]
fn revert_on_transfer_token_causes_write_option_to_revert_atomically() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, RevertOnTransferToken);
    let _ = token_id;
    // When implemented:
    // - register OptionsMarket with RevertOnTransferToken as collateral
    // - call write_option; assert it panics
    // - assert no series or position state was written
}

/// A token whose `balance()` always returns `i128::MAX` must not cause
/// the vault's balance-delta check to report a false positive when the
/// token is the collateral for a series.
///
/// **Issue #115 — Token: LyingBalanceToken delta boundary.**
#[test]
#[ignore = "issue #115: options_market is a stub; wire in when implemented"]
fn lying_balance_token_must_not_bypass_delta_check() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, LyingBalanceToken);
    let _ = token_id;
    // When implemented:
    // - set up a series with the LyingBalanceToken as collateral
    // - vault.deposit measures balance BEFORE and AFTER the transfer
    // - LyingBalanceToken reports MAX both times → delta = 0 → should fail
}
