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
    /// Symbol has not been registered via `register_symbol`.
    SymbolNotRegistered = 11,
    /// Symbol is already in the registry.
    SymbolAlreadyRegistered = 12,
    /// Registering would exceed `MAX_SYMBOLS`.
    TooManySymbols = 13,
    /// Symbol is not in canonical form: 1–12 chars of `A-Z`, `0-9`, `_`.
    InvalidSymbol = 14,
    /// `SymbolMeta` out of bounds (zero staleness override or
    /// `decimals > MAX_DECIMALS`).
    InvalidSymbolMeta = 15,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
