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
    /// Token is not on the vault allowlist.
    TokenNotAllowed = 7,
    /// Caller is not a registered integrator.
    UnregisteredIntegrator = 8,
    InsufficientVaultBalance = 9,
    /// An admin entrypoint was called before `initialize`.
    NotInitialized = 10,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
