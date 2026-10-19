use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    /// `initialize` was called twice. The timelock has no admin afterwards,
    /// so its setup is one-shot.
    AlreadyInitialized = 1,
    /// The caller does not hold the role the entrypoint requires.
    Unauthorized = 2,
    /// The delay is shorter than `min_delay`, or `min_delay` itself is below
    /// the documented floor.
    InsufficientDelay = 3,
    /// An operation with this id is already scheduled (or already done).
    AlreadyScheduled = 4,
    /// The operation is not ready yet: the delay has not elapsed.
    NotReady = 5,
    /// The operation's predecessor has not executed.
    PredecessorNotDone = 6,
    /// There is no pending operation with this id: it was never scheduled,
    /// it was cancelled, or it already executed.
    NotPending = 7,
    /// A self-call named a function this timelock does not implement.
    UnknownSelfCall = 8,
    /// The operation carries no calls, or its call vectors disagree in length.
    EmptyOperation = 9,
    /// The operation was ready, but its grace period has passed.
    Expired = 10,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
