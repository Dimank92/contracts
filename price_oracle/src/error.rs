use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    FeederAlreadyAdded = 2,
    FeederNotFound = 3,
    NotAFeeder = 4,
    InvalidPrice = 5,
    ContractPaused = 6,
    InvalidStaleness = 7,
    TooManyFeeders = 8,
    Unauthorized = 9,
    InvalidMinReports = 10,
    /// `aggregate`: no feeder has a report inside the freshness window.
    NoFreshReports = 11,
    /// `aggregate`: fewer than `min_reports` fresh reports survived
    /// outlier filtering.
    QuorumNotMet = 12,
    /// `accept_admin` with no transfer pending.
    NoPendingAdmin = 13,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
