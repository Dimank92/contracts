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
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
    /// A state the contract guarantees by construction was missing (e.g. a
    /// storage entry that `initialize` always writes). Never expected on a
    /// correctly deployed contract; replaces a panic with a typed error.
    InternalInvariant = 90,
}
