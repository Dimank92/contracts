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
    // Report ingest and the read path (oracle_adapter).
    UnknownSymbol = 11,
    FutureTimestamp = 12,
    StaleReport = 13,
    InvalidDecimals = 14,
    PrecisionExceeded = 15,
    NoReports = 16,
    StalePrice = 17,
    BelowQuorum = 18,
    ArithmeticOverflow = 19,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
