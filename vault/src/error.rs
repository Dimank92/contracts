use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    InvalidAmount = 2,
    InsufficientEscrowBalance = 3,
    ContractPaused = 4,
    NoUntaggedFunds = 5,
    Unauthorized = 6,
    TokenNotAllowed = 7,
    UnregisteredIntegrator = 8,
    InsufficientVaultBalance = 9,
}
