use soroban_sdk::contracterror;

/// Codes are append-only: never renumber or reuse one (docs/errors.md).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    /// The contract is already initialized.
    /// Cause: calling `initialize` a second time.
    AlreadyInitialized = 1,
    /// The amount must be above zero.
    /// Cause: `create_stream` with a non-positive total, or `withdraw` of 0 or less.
    InvalidAmount = 2,
    /// The schedule is invalid.
    /// Cause: `create_stream` with `start >= end`, or a cliff before `start` or after `end`.
    InvalidSchedule = 3,
    /// No stream has this id.
    /// Cause: an entrypoint called with a `stream_id` that was never created.
    StreamNotFound = 4,
    /// The caller may not cancel this stream.
    /// Cause: `cancel` from an address that is neither the stream's funder nor governance.
    Unauthorized = 5,
    /// The recipient has less accrued than requested.
    /// Cause: `withdraw` for more than `balance_of(stream_id).0`.
    InsufficientBalance = 6,
    /// The stream was cancelled.
    /// Cause: `cancel` or `transfer` on an already-cancelled stream.
    StreamCanceled = 7,
    /// The contract isn't initialized.
    /// Cause: calling an entrypoint before `initialize`.
    NotInitialized = 8,
    /// An arithmetic result doesn't fit its type.
    /// Cause: a stream total so large that `total × elapsed` exceeds `i128::MAX`, or the stream counter overflowing.
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
}
