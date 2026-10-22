//! Storage-schema migration steps for this contract. The framework
//! (version checks, ordering, pagination, the in-progress guard) lives in
//! `zenith_common::migrations`; this file only registers steps. See
//! docs/migrations.md for how to author one.
//!
//! Any signer may trigger `migrate`: the steps it runs are code in a
//! wasm that every signer approved via `approve_upgrade` (upgrade.rs), so
//! running them grants no power the upgrade didn't already.
//!
//! NOTE: lib.rs and types.rs are empty on main (emptied by #150). Once
//! they're restored, add `mod migrations;` to lib.rs, `init_schema` to
//! `initialize`, and call `migrations::require_current` from the
//! state-changing entrypoints (`approve`, `revoke`, `execute`, ...).

use soroban_sdk::{contractimpl, panic_with_error, Address, Env, Vec};
use zenith_common::migrations::{self, MigrationErrors, Step};

use crate::error::Error;
use crate::types::DataKey;
use crate::{ttl, Multisig};

/// The layout this wasm reads and writes. Raise it by one in the same PR
/// that adds the matching arm to `step`.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

pub const ERRORS: MigrationErrors<Error> = MigrationErrors {
    version_mismatch: Error::SchemaVersionMismatch,
    invalid_target: Error::InvalidMigrationTarget,
    in_progress: Error::MigrationInProgress,
    not_in_progress: Error::NoMigrationInProgress,
    invalid_batch: Error::InvalidMigrationBatch,
};

/// Step registry, keyed by the version each step migrates **to**. Layout
/// 1 is the baseline, so there are no steps yet. Add one arm per version:
///
/// ```ignore
/// 2 => v2(env, cursor, limit),
/// ```
///
/// A step over unbounded data processes items `[cursor, cursor + limit)`
/// and returns `Step::More(cursor + processed)` until it's done.
pub fn step(env: &Env, to: u32, cursor: u32, limit: u32) -> Step {
    let _ = (cursor, limit);
    #[allow(clippy::match_single_binding)]
    match to {
        _ => panic_with_error!(env, Error::InvalidMigrationTarget),
    }
}

/// The `MigrationInProgress` guard for state-changing entrypoints.
pub fn require_current(env: &Env) {
    migrations::require_schema_current(env, CURRENT_SCHEMA_VERSION, Error::MigrationInProgress);
}

#[contractimpl]
impl Multisig {
    /// Runs the registered steps `from+1 ..= to`. Any signer. Returns the
    /// version reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, signer: Address, from: u32, to: u32) -> u32 {
        ttl::extend_instance(&env);
        signer.require_auth();
        let signers: Vec<Address> = env.storage().instance().get(&DataKey::Signers).unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
        if !signers.contains(&signer) {
            panic_with_error!(&env, Error::NotASigner);
        }
        migrations::migrate(&env, from, to, CURRENT_SCHEMA_VERSION, step, &ERRORS)
    }

    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by `migrate` and the steps are code in
    /// this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> u32 {
        ttl::extend_instance(&env);
        migrations::migrate_batch(&env, cursor, limit, step, &ERRORS)
    }

    pub fn schema_version(env: Env) -> u32 {
        ttl::extend_instance(&env);
        migrations::schema_version(&env)
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        ttl::extend_instance(&env);
        migrations::migration_state(&env)
    }
}
