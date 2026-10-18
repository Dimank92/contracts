use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    InvalidAmount = 2,
    InsufficientEscrowBalance = 3,
    ContractPaused = 4,
    NoUntaggedFunds = 5,
    Unauthorized = 6,
    /// The token moved less into the vault than `deposit` asked for
    /// (fee-on-transfer, a no-op transfer, or a lying `balance`).
    DeltaMismatch = 7,
    /// `accept_admin` with no transfer pending.
    NoPendingAdmin = 8,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
