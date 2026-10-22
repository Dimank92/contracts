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
    Unauthorized = 9,
    ZeroAddress = 10,
    PoolAlreadySet = 11,
    InvalidPool = 12,
}