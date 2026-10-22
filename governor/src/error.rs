use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    InvalidProposal = 2,
    ProposalExists = 3,
    ProposalNotFound = 4,
    BelowProposalThreshold = 5,
    InvalidState = 6,
    AlreadyVoted = 7,
    InvalidSupport = 8,
    OutOfBounds = 9,
    Unauthorized = 10,
    NotQueued = 11,
    NotSucceeded = 12,
    ProposalExpired = 13,
    ProposalCanceled = 14,
    ExecutionFailed = 15,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}