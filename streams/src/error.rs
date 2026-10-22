use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    InvalidAmount = 2,
    InvalidSchedule = 3,
    StreamNotFound = 4,
    Unauthorized = 5,
    InsufficientBalance = 6,
    StreamCanceled = 7,
    StreamCompleted = 8,
    Overflow = 9,
    InvalidRate = 10,
    /// The contract's balance grew by less than the amount pulled.
    TransferShortfall = 11,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
