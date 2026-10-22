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
    NotInitialized = 11,
    InvalidAmount = 12,
    InvalidDisputeWindow = 13,
    NoBond = 14,
    InsufficientBond = 15,
    DisputeAlreadyOpen = 16,
    NoDispute = 17,
    DisputeWindowOpen = 18,
    DisputeWindowClosed = 19,
    InvalidEvidence = 20,
    ReportAlreadySlashed = 21,
    FeederDisputed = 22,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
