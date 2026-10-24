use soroban_sdk::contracterror;

/// Codes are append-only: never renumber or reuse one (docs/errors.md).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    /// The governor is already initialized.
    /// Cause: calling `initialize` a second time.
    AlreadyInitialized = 1,
    /// The proposal's action lists are invalid.
    /// Cause: `propose` with no actions, more than `MAX_ACTIONS`, or `targets`/`fns`/`args` of different lengths.
    InvalidProposal = 2,
    /// A proposal with the same id already exists.
    /// Cause: `propose` with the same actions and description hash as an existing proposal.
    ProposalExists = 3,
    /// No proposal has this id.
    /// Cause: an entrypoint called with an id `propose` never returned.
    ProposalNotFound = 4,
    /// The proposer's voting power is below the proposal threshold.
    /// Cause: `propose` from an account with fewer past votes than `proposal_threshold`.
    BelowProposalThreshold = 5,
    /// The proposal isn't in a state that allows this call.
    /// Cause: voting outside `Active`, queueing a proposal that hasn't `Succeeded`, or cancelling a finished one.
    InvalidState = 6,
    /// This voter already voted on this proposal.
    /// Cause: a second `cast_vote` / `cast_vote_by_sig` by the same voter.
    AlreadyVoted = 7,
    /// The vote's `support` value is invalid.
    /// Cause: `support` other than 0 (Against), 1 (For) or 2 (Abstain).
    InvalidSupport = 8,
    /// A governance parameter is outside its allowed range.
    /// Cause: `initialize` or a `set_*` call with a value outside the documented bounds.
    OutOfBounds = 9,
    /// The caller may not cancel this proposal.
    /// Cause: `cancel` by someone other than the proposer of a `Pending` proposal while the proposer still meets the threshold.
    Unauthorized = 10,
    /// The governor isn't initialized.
    /// Cause: calling an entrypoint before `initialize`.
    NotInitialized = 11,
    /// An arithmetic result doesn't fit its type.
    /// Cause: vote tallies above `i128::MAX`, a quorum computation overflowing, or ledger arithmetic past `u32::MAX`.
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
