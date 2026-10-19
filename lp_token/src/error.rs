use soroban_sdk::contracterror;

/// Codes are append-only: never renumber or reuse one (docs/errors.md).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    /// The token already has a pool.
    /// Cause: calling `initialize` a second time.
    AlreadyInitialized = 1,
    /// The token has no pool yet.
    /// Cause: `mint`, `pool` or a metadata read before `initialize`.
    NotInitialized = 2,
    /// Amounts can't be negative.
    /// Cause: a transfer, approval, mint or burn with a negative amount.
    NegativeAmount = 3,
    /// The balance is too low.
    /// Cause: transferring or burning more than the holder's balance.
    InsufficientBalance = 4,
    /// The allowance is too low.
    /// Cause: `transfer_from` / `burn_from` for more than the spender's unexpired allowance.
    InsufficientAllowance = 5,
    /// The allowance expiration is in the past.
    /// Cause: `approve` with a nonzero amount and an `expiration_ledger` before the current ledger.
    InvalidExpiration = 6,
    /// The decimals don't fit a `u8`.
    /// Cause: `initialize` with `decimal` above 255.
    InvalidDecimals = 7,
    /// A balance would exceed `i128::MAX`.
    /// Cause: minting or transferring into a balance close to `i128::MAX`.
    Overflow = 8,
}
