use soroban_sdk::contracterror;

/// Codes are append-only: never renumber or reuse one (docs/errors.md).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    /// The registry already has a timelock.
    /// Cause: calling `initialize` a second time.
    AlreadyInitialized = 1,
    /// No parameter is defined under this key.
    /// Cause: `set_param`, `get_value` or a bounds call for a key `define_param` never created.
    ParamNotFound = 2,
    /// A parameter already exists under this key.
    /// Cause: `define_param` for a key that is already defined; use `set_param`.
    ParamAlreadyDefined = 3,
    /// The value is outside the parameter's `[min, max]` bounds.
    /// Cause: `set_param` / `define_param` with an out-of-range value, or bounds that exclude the current value.
    OutOfBounds = 4,
    /// `min` is greater than `max`.
    /// Cause: `define_param` or `propose_bounds` with inverted bounds.
    InvalidBounds = 5,
    /// No bounds change is queued for this key.
    /// Cause: `execute_bounds` / `cancel_bounds` without a prior `propose_bounds`.
    NoPendingBounds = 6,
    /// The queued bounds change isn't executable yet.
    /// Cause: `execute_bounds` before `BOUNDS_CHANGE_DELAY` has passed since `propose_bounds`.
    BoundsDelayNotElapsed = 7,
    /// The registry has no timelock yet.
    /// Cause: calling a timelock-gated entrypoint before `initialize`.
    NotInitialized = 8,
    /// An arithmetic result doesn't fit its type.
    /// Cause: a bounds `eta` past `u64::MAX`, or the version counter overflowing.
    MathOverflow = 9,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    /// `from` isn't the stored schema version.
    /// Cause: `migrate(from, to)` with a stale `from`, including repeating a finished step.
    SchemaVersionMismatch = 100,
    /// The migration target is invalid.
    /// Cause: `migrate` with `to <= from`, or `to` beyond what the running wasm can migrate to.
    InvalidMigrationTarget = 101,
    /// A storage migration is unfinished, so state-changing entrypoints are closed.
    /// Cause: a paginated `migrate` still needs `migrate_batch`, or the wasm was upgraded and `migrate` hasn't run.
    MigrationInProgress = 102,
    /// No paginated migration is in flight.
    /// Cause: `migrate_batch` when nothing is in progress.
    NoMigrationInProgress = 103,
    /// The migration batch is invalid.
    /// Cause: `migrate_batch` with a cursor other than where the last batch stopped, or a limit of 0 or above `MAX_MIGRATION_BATCH`.
    InvalidMigrationBatch = 104,
    /// A state the contract guarantees by construction was missing (e.g. a
    /// storage entry that `initialize` always writes). Never expected on a
    /// correctly deployed contract; replaces a panic with a typed error.
    InternalInvariant = 90,
}
