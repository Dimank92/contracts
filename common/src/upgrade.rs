//! Upgrade safety: a governance-registered wasm hash allowlist with a
//! delay, a post-upgrade self-check that keeps the contract closed until
//! it passes, and the **pinned** multisig. See docs/upgrades.md.
//!
//! The lifecycle of every upgrade:
//!
//! 1. **Propose.** The contract's upgrade authority calls
//!    `propose_wasm(hash)`, which allowlists `hash` with
//!    `not_before = now + delay` and emits `upgrade_proposed(hash,
//!    not_before)`, so users and monitors can fetch and inspect the code
//!    before it can land.
//! 2. **Install.** Once `not_before` has passed, `upgrade(hash)` (same
//!    authority) consumes the allowlist entry, raises the post-upgrade
//!    guard, and swaps the wasm. Installing a hash that was never
//!    allowlisted, or before it matured, fails.
//! 3. **Migrate.** If the new wasm raises the schema version, `migrate`
//!    runs (zenith_common::migrations).
//! 4. **Check.** Anyone calls `post_upgrade_check()` on the new code. It
//!    requires the schema to be current, then runs the contract's own
//!    invariant and config checks. If they pass, the guard is lowered and
//!    normal entrypoints reopen. If they fail, the failure is recorded,
//!    `upgrade_check_failed` is emitted and the contract stays closed
//!    (fail-closed, an automatic pause) until a fixed wasm is installed.
//!
//! Between 2 and 4 every user-facing entrypoint fails with
//! `UpgradeCheckPending`, the same way it fails with
//! `MigrationInProgress` while a migration is unfinished. Upgrade,
//! migration and check entrypoints stay available, so a botched upgrade
//! can always be fixed by proposing and installing another wasm.
//!
//! Note that steps 1 and 2 run in the **old** wasm: the allowlist and the
//! guard are enforced by the code being replaced. Upgrading away from a
//! wasm that predates this module installs without them.
//!
//! **Pinned multisig.** `upgrade_via_multisig(action_id, ...)` doesn't
//! take a `multisig_contract` argument the way older `_via_multisig`
//! entrypoints do (docs/governance-handover.md, "Known caveat"). It only
//! trusts the multisig the admin pinned with `set_upgrade_multisig`, and
//! `action_id` must equal `upgrade_action_id(hash)`, derived from this
//! contract's address and the wasm hash, so an approval covers one wasm
//! on one contract.

use soroban_sdk::{contracttype, xdr::ToXdr, Address, Bytes, BytesN, Env, IntoVal, Val};

use crate::{events, is_executable, ActionClass};

/// Delay between `propose_wasm` and `upgrade` on the normal path.
pub const UPGRADE_DELAY: u64 = 3 * 86_400;

/// Shortest delay any contract may use. Reserved for an emergency path
/// gated by a strictly higher approval threshold than the normal one
/// (docs/upgrades.md, "Emergency fixes").
pub const EMERGENCY_UPGRADE_DELAY: u64 = 6 * 3_600;

/// ~5s ledgers, for sizing the allowlist entry's TTL.
const DAY_IN_LEDGERS: u32 = 17_280;

#[contracttype(export = false)]
#[derive(Clone)]
enum UpgradeKey {
    PinnedMultisig,
    /// wasm hash -> `WasmProposal`, persistent, until installed or cancelled.
    Allowlisted(BytesN<32>),
    /// Present from install until `post_upgrade_check` passes.
    PendingCheck,
}

/// An allowlisted wasm hash.
#[contracttype(export = false)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WasmProposal {
    pub proposed_at: u64,
    /// `upgrade(hash)` is rejected before this timestamp.
    pub not_before: u64,
    /// Proposed through the contract's emergency (shorter delay, higher
    /// threshold) path.
    pub emergency: bool,
}

/// The post-upgrade guard, from install until the self-check passes.
#[contracttype(export = false)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingCheck {
    /// The wasm that was installed.
    pub wasm_hash: BytesN<32>,
    pub installed_at: u64,
    /// Set once `post_upgrade_check` has run and failed. The guard stays
    /// up until a later check passes or another wasm is installed.
    pub failed: bool,
    /// Error code (in the contract's own registry) of the failed check,
    /// zero until a check fails.
    pub reason: u32,
}

/// Each contract's own codes for the failures below (docs/errors.md).
#[derive(Clone, Copy)]
pub struct UpgradeErrors<E> {
    pub unauthorized: E,
    pub not_initialized: E,
    /// `upgrade` / `cancel_wasm` for a hash that isn't allowlisted.
    pub not_proposed: E,
    /// `upgrade` before the hash's `not_before`.
    pub not_matured: E,
    /// `propose_wasm` for a hash that is already allowlisted.
    pub already_proposed: E,
    /// The post-upgrade guard is up (returned by `require_checked`).
    pub check_pending: E,
    /// `post_upgrade_check` with no upgrade awaiting a check.
    pub no_check_pending: E,
    /// `post_upgrade_check` while a migration is unfinished.
    pub migration_in_progress: E,
}

/// Who authorizes an upgrade on contracts with an admin and a pinned
/// multisig.
pub enum UpgradeAuth {
    /// The address stored under the contract's admin key signs.
    Admin,
    /// The pinned multisig reports `action_id` executable as `Critical`.
    PinnedMultisig(u64),
}

/// Checks `auth` for `wasm_hash`: the admin's signature, or the pinned
/// multisig's `Critical` approval of `upgrade_action_id(wasm_hash)`.
pub fn require_auth<K, E>(
    env: &Env,
    admin_key: &K,
    auth: &UpgradeAuth,
    wasm_hash: &BytesN<32>,
    errs: &UpgradeErrors<E>,
) -> Result<(), E>
where
    K: IntoVal<Env, Val>,
    E: Copy,
{
    match auth {
        UpgradeAuth::Admin => {
            crate::get_admin(env, admin_key, errs.not_initialized)?.require_auth()
        }
        UpgradeAuth::PinnedMultisig(action_id) => {
            let multisig = pinned_multisig(env).ok_or(errs.unauthorized)?;
            if *action_id != upgrade_action_id(env, wasm_hash)
                || !is_executable(env, &multisig, *action_id, ActionClass::Critical)
            {
                return Err(errs.unauthorized);
            }
        }
    }
    Ok(())
}

/// Pins (or re-pins) the multisig `upgrade_via_multisig` trusts. The
/// caller must already have checked admin auth.
pub fn pin_multisig(env: &Env, multisig: &Address) {
    env.storage()
        .instance()
        .set(&UpgradeKey::PinnedMultisig, multisig);
    events::upgrade_multisig_pinned(env, multisig.clone());
}

pub fn pinned_multisig(env: &Env) -> Option<Address> {
    env.storage().instance().get(&UpgradeKey::PinnedMultisig)
}

/// The multisig `action_id` that authorizes upgrading *this* contract to
/// `new_wasm_hash`: the first 8 bytes (big-endian) of
/// `sha256("zenith.upgrade" || xdr(this_contract) || new_wasm_hash)`.
pub fn upgrade_action_id(env: &Env, new_wasm_hash: &BytesN<32>) -> u64 {
    let mut preimage = Bytes::from_slice(env, b"zenith.upgrade");
    preimage.append(&env.current_contract_address().to_xdr(env));
    preimage.append(&Bytes::from_array(env, &new_wasm_hash.to_array()));
    let digest = env.crypto().sha256(&preimage).to_array();
    let mut id = [0u8; 8];
    id.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(id)
}

// ── Allowlist ────────────────────────────────────────────────────────────────

/// Allowlists `wasm_hash`, installable from `now + delay`. The caller
/// must already have checked the upgrade authority (and, for an
/// emergency proposal, the higher threshold). Returns `not_before`.
pub fn propose_wasm<E: Copy>(
    env: &Env,
    wasm_hash: &BytesN<32>,
    delay: u64,
    emergency: bool,
    errs: &UpgradeErrors<E>,
) -> Result<u64, E> {
    let key = UpgradeKey::Allowlisted(wasm_hash.clone());
    if env.storage().persistent().has(&key) {
        return Err(errs.already_proposed);
    }
    let now = env.ledger().timestamp();
    let not_before = now.saturating_add(delay);
    env.storage().persistent().set(
        &key,
        &WasmProposal {
            proposed_at: now,
            not_before,
            emergency,
        },
    );
    // The entry must outlive the delay: assume ledgers close no slower
    // than every 3s, plus a 30-day window to actually install.
    let ledgers = (delay / 3 + 1)
        .saturating_add(30 * DAY_IN_LEDGERS as u64)
        .min(env.storage().max_ttl() as u64) as u32;
    env.storage()
        .persistent()
        .extend_ttl(&key, ledgers, ledgers);
    events::upgrade_proposed(env, wasm_hash.clone(), not_before, emergency);
    Ok(not_before)
}

/// Removes `wasm_hash` from the allowlist. Caller checks authority.
pub fn cancel_wasm<E: Copy>(
    env: &Env,
    wasm_hash: &BytesN<32>,
    errs: &UpgradeErrors<E>,
) -> Result<(), E> {
    let key = UpgradeKey::Allowlisted(wasm_hash.clone());
    if !env.storage().persistent().has(&key) {
        return Err(errs.not_proposed);
    }
    env.storage().persistent().remove(&key);
    events::upgrade_cancelled(env, wasm_hash.clone());
    Ok(())
}

pub fn proposed_wasm(env: &Env, wasm_hash: &BytesN<32>) -> Option<WasmProposal> {
    env.storage()
        .persistent()
        .get(&UpgradeKey::Allowlisted(wasm_hash.clone()))
}

/// Installs an allowlisted, matured `wasm_hash`: consumes the allowlist
/// entry, raises the post-upgrade guard and swaps this contract's
/// executable (address and storage are kept). Caller checks authority.
pub fn install<E: Copy>(
    env: &Env,
    wasm_hash: &BytesN<32>,
    errs: &UpgradeErrors<E>,
) -> Result<(), E> {
    let key = UpgradeKey::Allowlisted(wasm_hash.clone());
    let proposal: WasmProposal = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(errs.not_proposed)?;
    let now = env.ledger().timestamp();
    if now < proposal.not_before {
        return Err(errs.not_matured);
    }
    env.storage().persistent().remove(&key);
    env.storage().instance().set(
        &UpgradeKey::PendingCheck,
        &PendingCheck {
            wasm_hash: wasm_hash.clone(),
            installed_at: now,
            failed: false,
            reason: 0,
        },
    );
    env.deployer()
        .update_current_contract_wasm(wasm_hash.clone());
    events::upgraded(
        env,
        wasm_hash.clone(),
        crate::migrations::schema_version(env),
    );
    Ok(())
}

// ── Post-upgrade check ───────────────────────────────────────────────────────

pub fn pending_check(env: &Env) -> Option<PendingCheck> {
    env.storage().instance().get(&UpgradeKey::PendingCheck)
}

/// The post-upgrade guard for user-facing entrypoints: `Err(err)` from
/// install until `post_upgrade_check` passes.
pub fn require_checked<E>(env: &Env, err: E) -> Result<(), E> {
    if env.storage().instance().has(&UpgradeKey::PendingCheck) {
        return Err(err);
    }
    Ok(())
}

/// Runs the self-check on the newly installed code. Permissionless.
///
/// `Err(migration_in_progress)` (nothing recorded) while a migration is
/// in flight or the stored schema lags `current_schema`: run `migrate`
/// first. Otherwise runs `check`, the contract's invariant and config
/// checks. On success the guard is lowered and `Ok(true)` returned. On
/// failure the error's code is recorded, `upgrade_check_failed` is
/// emitted and `Ok(false)` returned, so the record persists (an `Err`
/// would revert it) and the contract stays closed.
pub fn post_upgrade_check<E, F>(
    env: &Env,
    current_schema: u32,
    check: F,
    errs: &UpgradeErrors<E>,
) -> Result<bool, E>
where
    E: Copy + Into<soroban_sdk::Error>,
    F: FnOnce(&Env) -> Result<(), E>,
{
    let mut pending = pending_check(env).ok_or(errs.no_check_pending)?;
    crate::migrations::require_schema_current(env, current_schema, errs.migration_in_progress)?;
    match check(env) {
        Ok(()) => {
            env.storage().instance().remove(&UpgradeKey::PendingCheck);
            events::upgrade_checked(env, pending.wasm_hash, current_schema);
            Ok(true)
        }
        Err(e) => {
            let reason = e.into().get_code();
            pending.failed = true;
            pending.reason = reason;
            env.storage()
                .instance()
                .set(&UpgradeKey::PendingCheck, &pending);
            events::upgrade_check_failed(env, pending.wasm_hash, reason);
            Ok(false)
        }
    }
}
