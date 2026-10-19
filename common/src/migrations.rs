//! Versioned storage-schema migrations, shared by every Zenith contract.
//!
//! Each contract stores a `SchemaVersion(u32)` in instance storage and
//! compiles in a `CURRENT_SCHEMA_VERSION` plus a step registry (its own
//! `migrations.rs`). A step is keyed by the version it migrates **to**:
//! step `n` turns a layout-`n-1` contract into a layout-`n` one. Steps are
//! code in the new wasm, so upgrading is always `upgrade(new_wasm_hash)`
//! followed by `migrate(old, new)` — see docs/migrations.md.
//!
//! Guarantees enforced here, once, for every contract:
//! - `migrate(from, to)` only runs when the stored version equals `from`,
//!   so a completed step can never be repeated (the stored version has
//!   moved past it).
//! - Steps run strictly in order `from+1 ..= to`, each recording its own
//!   version bump, so no step can be skipped.
//! - `to` may not exceed the running wasm's `CURRENT_SCHEMA_VERSION`.
//! - A step that is too large for one transaction reports `More(cursor)`;
//!   the migration is then *in progress* and must be finished with
//!   `migrate_batch(cursor, limit)`, which only accepts the exact cursor
//!   the previous batch stopped at (no replayed or skipped pages).
//! - `require_schema_current` blocks normal entrypoints while a migration
//!   is in progress **or** while the stored version lags the code (the
//!   wasm was upgraded but `migrate` hasn't run), so a half-migrated
//!   contract never serves traffic.
//!
//! Authorization is the caller's job: each contract checks its own admin /
//! multisig / timelock before `migrate`. `migrate_batch` is meant to be
//! exposed permissionlessly: it can only advance a migration an
//! authorized `migrate` already started.

use soroban_sdk::{contracttype, panic_with_error, Env, Symbol};

/// Version assumed for a contract deployed before this framework existed
/// (no `SchemaVersion` key in storage), and the version every contract
/// writes at `initialize`. Layout 1 is "the layout as of this framework".
pub const BASELINE_SCHEMA_VERSION: u32 = 1;

/// Page size `migrate` uses for every step it runs.
pub const DEFAULT_MIGRATION_BATCH: u32 = 50;

/// Upper bound on `migrate_batch`'s `limit`, to keep one page inside
/// Soroban's per-transaction resource limits.
pub const MAX_MIGRATION_BATCH: u32 = 200;

/// Instance-storage keys owned by this module. No contract's `DataKey`
/// has variants with these names, so the encodings can't collide.
#[contracttype(export = false)]
#[derive(Clone)]
enum MigrationKey {
    SchemaVersion,
    /// Present only while a paginated step is mid-flight.
    MigrationState,
}

#[contracttype(export = false)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MigrationState {
    /// Version the running `migrate` call was asked to reach.
    target: u32,
    /// Where the current step (`schema_version() + 1`) resumes.
    cursor: u32,
}

/// What a step reports back after processing up to `limit` items
/// starting at `cursor`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    /// The step is complete; the schema version is bumped to its target.
    Done,
    /// More work remains; resume at this cursor. Must be strictly greater
    /// than the cursor the step was called with.
    More(u32),
}

/// A contract's step registry: `(env, to_version, cursor, limit)`. Called
/// only for `to_version` in `BASELINE_SCHEMA_VERSION+1 ..= current`; must
/// panic for a version it has no step for.
pub type StepFn = fn(&Env, u32, u32, u32) -> Step;

/// Each contract's own error codes for the failure cases below, so errors
/// show up in that contract's spec rather than as foreign codes.
#[derive(Clone, Copy)]
pub struct MigrationErrors<E> {
    /// `from` isn't the stored schema version (includes repeating a step).
    pub version_mismatch: E,
    /// `to <= from`, or `to` is beyond what this wasm knows how to reach.
    pub invalid_target: E,
    /// A migration is in progress (or pending after an upgrade).
    pub in_progress: E,
    /// `migrate_batch` was called with nothing in progress.
    pub not_in_progress: E,
    /// Wrong cursor, `limit` of 0 / above `MAX_MIGRATION_BATCH`, or a step
    /// that failed to advance its cursor.
    pub invalid_batch: E,
}

/// Writes the schema version at `initialize`.
pub fn init_schema(env: &Env, version: u32) {
    env.storage()
        .instance()
        .set(&MigrationKey::SchemaVersion, &version);
}

pub fn schema_version(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&MigrationKey::SchemaVersion)
        .unwrap_or(BASELINE_SCHEMA_VERSION)
}

/// `(target, cursor)` of the in-flight paginated step, if any.
pub fn migration_state(env: &Env) -> Option<(u32, u32)> {
    load_state(env).map(|s| (s.target, s.cursor))
}

pub fn is_migrating(env: &Env) -> bool {
    env.storage().instance().has(&MigrationKey::MigrationState)
}

/// The `MigrationInProgress` guard for normal entrypoints: panics with
/// `err` while a paginated migration is mid-flight or while the stored
/// schema is older than the running code's `current`.
pub fn require_schema_current<E>(env: &Env, current: u32, err: E)
where
    E: Into<soroban_sdk::Error>,
{
    if is_migrating(env) || schema_version(env) != current {
        panic_with_error!(env, err);
    }
}

/// Runs steps `from+1 ..= to` in order. Returns the schema version
/// reached, which is `to` unless a step paginated (then the migration is
/// left in progress for `migrate_batch`).
pub fn migrate<E>(
    env: &Env,
    from: u32,
    to: u32,
    current: u32,
    step: StepFn,
    errs: &MigrationErrors<E>,
) -> u32
where
    E: Copy + Into<soroban_sdk::Error>,
{
    if is_migrating(env) {
        panic_with_error!(env, errs.in_progress);
    }
    if schema_version(env) != from {
        panic_with_error!(env, errs.version_mismatch);
    }
    if to <= from || to > current {
        panic_with_error!(env, errs.invalid_target);
    }
    run(env, from, to, 0, DEFAULT_MIGRATION_BATCH, step, errs)
}

/// Continues an in-progress migration from `cursor`, processing up to
/// `limit` items of the current step. `cursor` must be exactly where the
/// previous call stopped. Returns the schema version reached.
pub fn migrate_batch<E>(
    env: &Env,
    cursor: u32,
    limit: u32,
    step: StepFn,
    errs: &MigrationErrors<E>,
) -> u32
where
    E: Copy + Into<soroban_sdk::Error>,
{
    let state = load_state(env).unwrap_or_else(|| panic_with_error!(env, errs.not_in_progress));
    if cursor != state.cursor || limit == 0 || limit > MAX_MIGRATION_BATCH {
        panic_with_error!(env, errs.invalid_batch);
    }
    run(
        env,
        schema_version(env),
        state.target,
        cursor,
        limit,
        step,
        errs,
    )
}

fn run<E>(
    env: &Env,
    mut version: u32,
    target: u32,
    mut cursor: u32,
    limit: u32,
    step: StepFn,
    errs: &MigrationErrors<E>,
) -> u32
where
    E: Copy + Into<soroban_sdk::Error>,
{
    while version < target {
        let next = version + 1;
        match step(env, next, cursor, limit) {
            Step::Done => {
                init_schema(env, next);
                env.events()
                    .publish((Symbol::new(env, "schema_migrated"),), (version, next));
                version = next;
                cursor = 0;
            }
            Step::More(resume) => {
                if resume <= cursor {
                    panic_with_error!(env, errs.invalid_batch);
                }
                env.storage().instance().set(
                    &MigrationKey::MigrationState,
                    &MigrationState {
                        target,
                        cursor: resume,
                    },
                );
                env.events()
                    .publish((Symbol::new(env, "migration_progress"), next), resume);
                return version;
            }
        }
    }
    env.storage()
        .instance()
        .remove(&MigrationKey::MigrationState);
    version
}

fn load_state(env: &Env) -> Option<MigrationState> {
    env.storage().instance().get(&MigrationKey::MigrationState)
}

