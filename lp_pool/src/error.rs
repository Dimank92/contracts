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
    /// A state the contract guarantees by construction was missing (e.g. a
    /// storage entry that `initialize` always writes). Never expected on a
    /// correctly deployed contract; replaces a panic with a typed error.
    InternalInvariant = 90,
}
