use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    ProposalNotFound = 4,
    NotPending = 5,
    NotReady = 6,
    PredecessorNotExecuted = 7,
    InvalidPredecessor = 8,
    InvalidPagination = 9,
    TooManyPending = 10,
}
