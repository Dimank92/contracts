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
    /// Top-up or unwind attempted by an account that is not the position's writer.
    NotWriter = 35,
    /// Unwind attempted on a position that has already been unwound.
    PositionAlreadyUnwound = 36,
    /// Unwind attempted at or after the series expiry.
    UnwindAfterExpiry = 37,
    /// Collateral release blocked because the series is halted or in emergency state.
    SeriesHalted = 38,
    /// The oracle price is stale; the operation must fail rather than guess.
    StaleOraclePrice = 39,
}