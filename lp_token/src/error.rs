use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NegativeAmount = 3,
    InsufficientBalance = 4,
    InsufficientAllowance = 5,
    InvalidExpiration = 6,
    InvalidDecimals = 7,
    Overflow = 8,
    /// A state the contract guarantees by construction was missing (e.g. a
    /// storage entry that `initialize` always writes). Never expected on a
    /// correctly deployed contract; replaces a panic with a typed error.
    InternalInvariant = 90,
}
