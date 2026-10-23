//! Storage-schema migration steps for this contract. The framework
//! (version checks, ordering, pagination, the in-progress guard) lives in
//! `zenith_common::migrations`; this file only registers steps. See
//! docs/migrations.md for how to author one.
//!
//! `initialize` stamps `CURRENT_SCHEMA_VERSION`; an instance deployed
//! before that reads as `BASELINE_SCHEMA_VERSION` (1). Every ledger write
//! goes through `require_current`, so a stored version this wasm doesn't
//! know (older or newer) blocks deposits, withdrawals and transfers
//! instead of being reinterpreted, and `migrate` refuses any target above
//! `CURRENT_SCHEMA_VERSION`.
//!
//! Each `migrate`/`migrate_batch` call snapshots every per-token ledger
//! total before running steps and aborts the whole call with
//! `MigrationInvariantViolated` if any changed, so a step can never move
//! or lose escrow. Per-tag entries are only ever rewritten by a step that
//! also preserves their token's total.
//!
//! Migration is allowed while the vault is paused: it moves no funds and
//! is often what an operator pauses for.

use soroban_sdk::{contractimpl, panic_with_error, Address, BytesN, Env};
use zenith_common::migrations::{self, MigrationErrors, Step};
use zenith_common::upgrade::{self, UpgradeAuth};
use zenith_common::{require_admin_or_multisig, ActionClass, Auth};

use crate::error::Error;
use crate::types::DataKey;
use crate::{ledger_totals, tokens, ttl, Vault, VaultClient};

/// The layout this wasm reads and writes. Raise it by one in the same PR
/// that adds the matching arm to `step`.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

pub const ERRORS: MigrationErrors<Error> = MigrationErrors {
    version_mismatch: Error::SchemaVersionMismatch,
    invalid_target: Error::InvalidMigrationTarget,
    in_progress: Error::MigrationInProgress,
    not_in_progress: Error::NoMigrationInProgress,
    invalid_batch: Error::InvalidMigrationBatch,
};

/// Step registry, keyed by the version each step migrates **to**. Add one
/// arm per version:
///
/// ```ignore
/// 2 => v2(env, cursor, limit),
/// ```
///
/// A step over unbounded data processes items `[cursor, cursor + limit)`
/// and returns `Step::More(cursor + processed)` until it's done.
pub fn step(env: &Env, to: u32, cursor: u32, limit: u32) -> Step {
    let _ = (cursor, limit);
    match to {
        2 => v2(env),
        _ => panic_with_error!(env, Error::InvalidMigrationTarget),
    }
}

/// v1 -> v2: adds the `Tokens` index, defaulting to the default token. A
/// v1 vault only ever indexed its default token's total in instance
/// storage under a known key; any other token is added to the index by
/// its next ledger write.
fn v2(env: &Env) -> Step {
    if !env.storage().instance().has(&DataKey::Tokens) {
        env.storage().instance().set(&DataKey::Tokens, &tokens(env));
    }
    Step::Done
}

/// Runs `f` and aborts with `MigrationInvariantViolated` unless every
/// per-token ledger total is unchanged afterwards.
pub(crate) fn preserving_ledger(env: &Env, f: impl FnOnce() -> u32) -> u32 {
    let before = ledger_totals(env);
    let reached = f();
    let after = ledger_totals(env);
    for (i, total) in before.iter().enumerate() {
        if after.get(i as u32) != Some(total) {
            panic_with_error!(env, Error::MigrationInvariantViolated);
        }
    }
    reached
}

/// The `MigrationInProgress` guard for state-changing entrypoints.
pub fn require_current(env: &Env) {
    migrations::require_schema_current(env, CURRENT_SCHEMA_VERSION, Error::MigrationInProgress);
}

fn require_admin(env: &Env) {
    zenith_common::get_admin(env, &DataKey::Admin).require_auth();
}

/// Upgrade and migration entrypoints. A separate `#[contractimpl]` block
/// so the whole upgrade surface lives in one file.
#[contractimpl]
impl Vault {
    /// Swaps this contract's wasm, keeping its address and storage.
    /// Admin only. Follow with `migrate` if the new wasm raises
    /// `CURRENT_SCHEMA_VERSION` — until then state-changing entrypoints
    /// fail with `MigrationInProgress`.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        upgrade::upgrade(
            &env,
            &DataKey::Admin,
            UpgradeAuth::Admin,
            new_wasm_hash,
            Error::Unauthorized,
        );
    }

    /// Permissionless alternative to `upgrade`, authorized by the
    /// **pinned** multisig (`set_upgrade_multisig`) reporting `action_id`
    /// executable as `Critical`. `action_id` must equal
    /// `upgrade_action_id(new_wasm_hash)`, so the approval covers exactly
    /// this wasm on exactly this contract.
    pub fn upgrade_via_multisig(env: Env, action_id: u64, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        upgrade::upgrade(
            &env,
            &DataKey::Admin,
            UpgradeAuth::PinnedMultisig(action_id),
            new_wasm_hash,
            Error::Unauthorized,
        );
    }

    /// Pins the multisig `upgrade_via_multisig` and `migrate_via_multisig`
    /// trust. Admin only; re-pinning replaces it.
    pub fn set_upgrade_multisig(env: Env, multisig: Address) {
        ttl::extend_instance(&env);
        require_admin(&env);
        upgrade::pin_multisig(&env, &multisig);
    }

    pub fn get_upgrade_multisig(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        upgrade::pinned_multisig(&env)
    }

    /// The multisig action id that authorizes upgrading this contract to
    /// `new_wasm_hash` — what signers approve.
    pub fn upgrade_action_id(env: Env, new_wasm_hash: BytesN<32>) -> u64 {
        upgrade::upgrade_action_id(&env, &new_wasm_hash)
    }

    /// Runs the registered steps `from+1 ..= to`. Admin only. Returns the
    /// version reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, from: u32, to: u32) -> u32 {
        ttl::extend_instance(&env);
        require_admin(&env);
        preserving_ledger(&env, || {
            migrations::migrate(&env, from, to, CURRENT_SCHEMA_VERSION, step, &ERRORS)
        })
    }

    /// Permissionless alternative to `migrate`, authorized by the pinned
    /// multisig. `Emergency` class (no extra delay): the steps it runs
    /// were already timelocked as `Critical` with the upgrade that
    /// installed them, and the contract is blocked until they run.
    pub fn migrate_via_multisig(env: Env, action_id: u64, from: u32, to: u32) -> u32 {
        ttl::extend_instance(&env);
        let multisig = upgrade::pinned_multisig(&env)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Unauthorized));
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig, action_id),
            ActionClass::Emergency,
            Error::Unauthorized,
        );
        preserving_ledger(&env, || {
            migrations::migrate(&env, from, to, CURRENT_SCHEMA_VERSION, step, &ERRORS)
        })
    }

    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by `migrate` and the steps are code in
    /// this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> u32 {
        ttl::extend_instance(&env);
        preserving_ledger(&env, || {
            migrations::migrate_batch(&env, cursor, limit, step, &ERRORS)
        })
    }

    pub fn schema_version(env: Env) -> u32 {
        ttl::extend_instance(&env);
        migrations::schema_version(&env)
    }

    /// The storage layout version this instance is on (`schema_version`).
    pub fn version(env: Env) -> u32 {
        Self::schema_version(env)
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        ttl::extend_instance(&env);
        migrations::migration_state(&env)
    }
}
