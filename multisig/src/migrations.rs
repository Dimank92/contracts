//! Storage-schema migration steps for this contract. The framework
//! (version checks, ordering, pagination, the in-progress guard) lives in
//! `zenith_common::migrations`; this file only registers steps. See
//! docs/migrations.md for how to author one.
//!
//! Any signer may trigger `migrate`: the steps it runs are code in a
//! wasm that every signer approved via `approve_upgrade` (upgrade.rs), so
//! running them grants no power the upgrade didn't already.

use soroban_sdk::{contractimpl, Address, Env, Map};
use zenith_common::migrations::{self, MigrationErrors, Step};

use crate::error::Error;
use crate::types::DataKey;
use crate::{ttl, Multisig, MultisigClient};

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
pub fn step(env: &Env, to: u32, cursor: u32, limit: u32) -> Result<Step, Error> {
    let _ = (env, cursor, limit);
    #[allow(clippy::match_single_binding)]
    match to {
        _ => Err(Error::InvalidMigrationTarget),
    }
}

/// The guard for state-changing entrypoints: closed while a migration is
/// unfinished (`MigrationInProgress`) and from an upgrade until its
/// post-upgrade check passes (`UpgradeCheckPending`).
pub fn require_current(env: &Env) -> Result<(), Error> {
    migrations::require_schema_current(env, CURRENT_SCHEMA_VERSION, Error::MigrationInProgress)?;
    zenith_common::upgrade::require_checked(env, Error::UpgradeCheckPending)
}

#[contractimpl]
impl Multisig {
    /// Runs the registered steps `from+1 ..= to`. Any signer. Returns the
    /// version reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, signer: Address, from: u32, to: u32) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        signer.require_auth();
        let signers: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey::Signers)
            .ok_or(Error::NotInitialized)?;
        if !signers.contains_key(signer) {
            return Err(Error::NotASigner);
        }
        migrations::migrate(&env, from, to, CURRENT_SCHEMA_VERSION, step, &ERRORS)
    }

    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by `migrate` and the steps are code in
    /// this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        migrations::migrate_batch(&env, cursor, limit, step, &ERRORS)
    }

    pub fn schema_version(env: Env) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        Ok(migrations::schema_version(&env))
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Result<Option<(u32, u32)>, Error> {
        ttl::extend_instance(&env);
        Ok(migrations::migration_state(&env))
    }
}
