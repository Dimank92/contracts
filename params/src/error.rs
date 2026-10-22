use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    ParamNotFound = 2,
    ParamAlreadyDefined = 3,
    OutOfBounds = 4,
    InvalidBounds = 5,
    NoPendingBounds = 6,
    BoundsDelayNotElapsed = 7,
    // Namespaced parameter registry.
    TypeMismatch = 8,
    KeyNotFound = 9,
    InvalidNamespace = 10,
    InvalidKey = 11,
    InvalidCursor = 12,
    InvalidLimit = 13,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}