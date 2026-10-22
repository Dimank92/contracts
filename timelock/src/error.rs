use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    InsufficientDelay = 3,
    AlreadyScheduled = 4,
    NotReady = 5,
    /// The predecessor exists but has not executed yet.
    PredecessorNotExecuted = 6,
    NotPending = 7,
    UnknownSelfCall = 8,
    EmptyOperation = 9,
    /// A predecessor id that was never scheduled.
    ActionNotFound = 10,
    /// The predecessor, or one of its ancestors, was cancelled.
    PredecessorCancelled = 11,
    /// Scheduling would make the operation (transitively) its own predecessor.
    PredecessorCycle = 12,
    /// The predecessor chain is longer than `MAX_CHAIN_DEPTH`.
    PredecessorChainTooDeep = 13,
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
