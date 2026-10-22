use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidAmount = 3,
    InsufficientShares = 4,
    SeriesNotWhitelisted = 5,
    SeriesCapExceeded = 6,
    UtilizationCapExceeded = 7,
    InsufficientIdle = 8,
    PositionsStillOpen = 9,
    PositionNotFound = 10,
    InvalidUtilization = 11,
    TooManyPositions = 12,
    BelowMinimumDeposit = 13,
    DepositCapExceeded = 14,
    TotalCapacityExceeded = 15,
    InvalidFeeConfig = 16,
    Paused = 17,
    Unauthorized = 18,
}
