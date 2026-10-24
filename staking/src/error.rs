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
    /// Cause: `stake`, `request_unstake` or `notify_reward` with an amount of 0 or less.
    InvalidAmount = 2,
    /// The user has less stake than requested.
    /// Cause: `request_unstake` for more than the user's earning stake.
    InsufficientStake = 3,
    /// The user has no pending unstake.
    /// Cause: `withdraw` without a prior `request_unstake`, or after it was already withdrawn.
    NothingToWithdraw = 4,
    /// The unstake cooldown hasn't elapsed.
    /// Cause: `withdraw` before the pending unstake's `unlock_at`.
    CooldownActive = 5,
    /// The reward-token cap is reached.
    /// Cause: `notify_reward` with a new token when `MAX_REWARD_TOKENS` tokens are already tracked.
    TooManyRewardTokens = 6,
    /// The contract isn't initialized.
    /// Cause: calling an entrypoint before `initialize`.
    NotInitialized = 7,
    /// An arithmetic result doesn't fit its type.
    /// Cause: a stake, reward or accumulator value exceeding `i128::MAX`, or a cooldown pushing `unlock_at` past `u64::MAX`.
    MathOverflow = 8,
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
