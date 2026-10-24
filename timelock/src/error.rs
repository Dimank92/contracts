use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    /// `initialize` ran on a contract that was already initialized.
    AlreadyInitialized = 1,
    /// The caller does not hold the role the entrypoint requires.
    Unauthorized = 2,
    /// The requested delay is below `min_delay`.
    InsufficientDelay = 3,
    /// An operation with this id is already scheduled or already done.
    AlreadyScheduled = 4,
    /// The delay has not elapsed yet. An operation may run at exactly its
    /// ready-at.
    NotReady = 5,
    /// The operation's predecessor has not run (or was cancelled).
    PredecessorNotDone = 6,
    /// There is no pending operation with this id: it is unknown, or it has
    /// already run.
    NotPending = 7,
    /// A self-call named a function the timelock does not implement.
    UnknownSelfCall = 8,
    /// The batch has no calls in it.
    EmptyOperation = 9,

    // Schema migrations (zenith_common::migrations) — the same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
