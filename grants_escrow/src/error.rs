use soroban_sdk::contracterror;

/// Codes are append-only: never renumber or reuse one (docs/errors.md).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    /// The milestone list is invalid.
    /// Cause: `create_grant` with no milestones, more than `MAX_MILESTONES`, a non-positive amount, a deadline not in the future, or amounts summing past `i128::MAX`.
    InvalidMilestones = 1,
    /// The reviewer committee is invalid.
    /// Cause: `create_grant` with no reviewers, more than `MAX_REVIEWERS`, or a duplicate reviewer.
    InvalidReviewers = 2,
    /// The quorum is invalid.
    /// Cause: `create_grant` with a quorum of 0 or above the number of reviewers.
    InvalidQuorum = 3,
    /// The grantee can't review their own grant.
    /// Cause: `create_grant` listing the grantee among the reviewers.
    ReviewerIsGrantee = 4,
    /// No grant has this id.
    /// Cause: an entrypoint called with a `grant_id` that was never created.
    GrantNotFound = 5,
    /// The grant has no milestone at this index.
    /// Cause: `submit_milestone` / `approve_milestone` with `idx` past the last milestone.
    MilestoneNotFound = 6,
    /// The milestone isn't in the state this call needs.
    /// Cause: submitting a milestone that isn't `Pending`, or approving one that isn't `Submitted`.
    InvalidMilestoneState = 7,
    /// The milestone's deadline has passed.
    /// Cause: `submit_milestone` after the milestone's deadline.
    DeadlinePassed = 8,
    /// The caller isn't on this grant's reviewer committee.
    /// Cause: `approve_milestone` from an address not in the grant's `reviewers`.
    NotAReviewer = 9,
    /// This reviewer already approved this milestone.
    /// Cause: a reviewer calling `approve_milestone` twice for the same milestone.
    AlreadyApproved = 10,
    /// No milestone is reclaimable yet.
    /// Cause: `reclaim` before any unreleased milestone is `GRACE_PERIOD` past its deadline.
    NothingToReclaim = 11,
    /// An arithmetic result doesn't fit its type.
    /// Cause: the grant counter overflowing `u64`.
    MathOverflow = 12,
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
