use soroban_sdk::contracterror;

/// Codes are append-only: never renumber or reuse one (docs/errors.md).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    /// The pool is already initialized.
    /// Cause: calling `initialize` a second time.
    AlreadyInitialized = 1,
    /// The pool isn't initialized.
    /// Cause: calling an entrypoint before `initialize`.
    NotInitialized = 2,
    /// The amount is invalid, or a share/asset conversion overflowed.
    /// Cause: a non-positive amount, a negative cap or liability mark, or values so large that a conversion exceeds `i128::MAX`.
    InvalidAmount = 3,
    /// The user holds fewer shares than requested.
    /// Cause: `request_withdraw` for more shares than the user's balance.
    InsufficientShares = 4,
    /// The series isn't whitelisted.
    /// Cause: `write_option` into a series whose cap is zero (never set, or de-listed).
    SeriesNotWhitelisted = 5,
    /// The series' collateral cap would be exceeded.
    /// Cause: `write_option` locking more collateral in a series than its cap allows.
    SeriesCapExceeded = 6,
    /// The pool-wide utilization cap would be exceeded.
    /// Cause: `write_option` pushing `locked / (idle + locked)` above `max_utilization_bps`.
    UtilizationCapExceeded = 7,
    /// The pool doesn't have enough idle assets.
    /// Cause: `write_option` needing more collateral than is idle.
    InsufficientIdle = 8,
    /// Positions are still open.
    /// Cause: `process_epoch` before every open position has been reclaimed.
    PositionsStillOpen = 9,
    /// The pool has no open position with this id.
    /// Cause: `reclaim` for a position that was never written or was already reclaimed.
    PositionNotFound = 10,
    /// The utilization cap is invalid.
    /// Cause: `initialize` / `set_max_utilization` with 0 or more than 10_000 bps.
    InvalidUtilization = 11,
    /// The open-position list is full.
    /// Cause: `write_option` with `MAX_POSITIONS` positions already open.
    TooManyPositions = 12,
    /// The deposit is below the minimum.
    /// Cause: `deposit` of less than `MIN_DEPOSIT`.
    BelowMinimumDeposit = 13,
    /// A balance-delta computation doesn't fit its type.
    /// Cause: token balances moving by more than `i128::MAX` during a market call.
    MathOverflow = 14,
    /// A ticket refers to an epoch with no stored result.
    /// Cause: an epoch result that was never written or has been archived; restore it before claiming.
    EpochNotProcessed = 15,
}
