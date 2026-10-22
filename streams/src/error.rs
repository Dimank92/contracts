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
    /// The stream's schedule is fully vested / completed, so it can no
    /// longer be transferred to a new recipient.
    StreamCompleted = 8,
    /// `transfer_recipient` was called with the current recipient as the
    /// new recipient. Self-transfers are explicitly rejected.
    SelfTransfer = 9,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}