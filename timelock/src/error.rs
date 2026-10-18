use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    InvalidOperation = 3,
    DelayTooShort = 4,
    OperationExists = 5,
    OperationNotFound = 6,
    NotReady = 7,
    Expired = 8,
    AlreadyDone = 9,
    LockedIn = 10,
    UnknownSelfCall = 11,
    /// `schedule_after` named a predecessor that was never scheduled, or
    /// the operation itself.
    PredecessorNotFound = 12,
    /// The operation's predecessor hasn't executed yet (or was cancelled).
    PredecessorNotDone = 13,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
