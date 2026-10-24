use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    InvalidAmount = 2,
    InsufficientStake = 3,
    NothingToWithdraw = 4,
    CooldownActive = 5,
    TooManyRewardTokens = 6,
    // Admin surface (reward rate, cooldown, emergency controls).
    Unauthorized = 7,
    InvalidRewardRate = 8,
    InvalidCooldown = 9,
    StakesBlocked = 10,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}