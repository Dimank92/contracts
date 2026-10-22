use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    InvalidMilestones = 1,
    InvalidReviewers = 2,
    InvalidQuorum = 3,
    ReviewerIsGrantee = 4,
    GrantNotFound = 5,
    MilestoneNotFound = 6,
    InvalidMilestoneState = 7,
    DeadlinePassed = 8,
    NotAReviewer = 9,
    AlreadyApproved = 10,
    NothingToReclaim = 11,
    // New grants_escrow paths.
    Unauthorized = 12,
    GrantCanceled = 13,
    AlreadyClaimed = 14,
    InsufficientEscrow = 15,
    TokenNotAllowed = 16,
    InvalidAmount = 17,
    GrantNotActive = 18,
    EscrowTagMismatch = 19,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}