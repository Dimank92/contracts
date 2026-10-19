//! Storage-schema migration steps for this contract. The framework
//! (version checks, ordering, pagination, the in-progress guard) lives in
//! `zenith_common::migrations`; this file only registers steps. See
//! docs/migrations.md for how to author one.
//!
//! The timelock administers itself: there is no `migrate` entrypoint.
//! Instead `execute` dispatches a call targeting the timelock with
//! function `migrate` and args `(from: u32, to: u32)` to `self_migrate`
//! below, the same way it handles `update_delay` / `grant_role` /
//! `revoke_role` / `set_open_executor`.
//! A migration is therefore always a delayed, cancellable operation. It
//! must be its own operation, after the one that installs the new wasm:
//! a wasm swap only takes effect once the invocation that made it
//! returns, so a `migrate` self-call in the same `execute` would still run
//! the old code.

use soroban_sdk::{contractimpl, panic_with_error, Env};
use zenith_common::migrations::{self, MigrationErrors, Step};

use crate::error::Error;
use crate::Timelock;

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

/// Target of a timelock self-call `migrate(from, to)`; only reachable
/// through a ready operation in `execute`.
pub fn self_migrate(env: &Env, from: u32, to: u32) -> u32 {
    migrations::migrate(env, from, to, CURRENT_SCHEMA_VERSION, step, &ERRORS)
}

#[contractimpl]
impl Timelock {
    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by the delayed `migrate` self-call and
    /// the steps are code in this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> u32 {
        migrations::migrate_batch(&env, cursor, limit, step, &ERRORS)
    }

    pub fn schema_version(env: Env) -> u32 {
        migrations::schema_version(&env)
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        migrations::migration_state(&env)
    }
}
