use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    SeriesNotFound = 3,
    SeriesNotActive = 4,
    SeriesNotExpired = 5,
    PositionNotFound = 6,
    InsufficientPremium = 7,
    InsufficientCollateral = 8,
    AlreadyExercised = 9,
    AlreadySettled = 10,
    ExerciseWindowClosed = 11,
    ZeroContracts = 12,
    PriceNotSet = 13,
    NotInTheMoney = 14,
    WrongSide = 15,
    ExpiryTooSoon = 16,
    ContractPaused = 17,
    SeriesNotCancelled = 18,
    InvalidFeeRate = 19,
    TooManySeriesForUnderlying = 20,
    InsufficientPremiumPool = 21,
    InvalidSeriesParams = 22,
    InvalidBatchSize = 23,
    NothingToEscrow = 24,
    PremiumBelowMinimum = 25,
    NotEligibleForForfeiture = 26,
    DuplicateSeries = 27,
    InvalidSplitAmount = 28,
    /// The entry is not in a terminal state, or still carries a liability.
    NotPrunable = 29,
    /// The entry is terminal but `PRUNE_RETENTION` hasn't elapsed yet.
    RetentionNotElapsed = 30,
    /// `prune_series` on a series that still has unpruned positions.
    SeriesHasPositions = 31,
    /// `migrate_counters` hasn't finished since the upgrade.
    MigrationPending = 32,
    InvalidActiveSeriesCap = 33,
    InvalidPageLimit = 34,
    /// The series is halted: no new purchases or writes (#192).
    SeriesHalted = 35,
    /// `unhalt_series` on a series that is not halted.
    SeriesNotHalted = 36,
    /// Premium is non-positive or above `math::max_premium` (#192).
    PremiumOutOfBounds = 37,
    /// Arithmetic overflow while scaling a settlement payoff (#191).
    PayoffOverflow = 38,
    /// `settle_series` with no price oracle pinned (#191).
    PriceOracleNotSet = 39,
    /// A vault deposit didn't credit the series tag with exactly the
    /// amount sent (#158 post-condition).
    EscrowPostConditionFailed = 40,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}
