use soroban_sdk::contracterror;

/// Errors returned by the `timelock` contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum TimelockError {
    /// Contract has already been initialized.
    AlreadyInitialized = 1,
    /// Contract has not been initialized yet.
    NotInitialized = 2,
    /// Caller is not authorized to perform this action.
    Unauthorized = 3,
    /// The supplied minimum delay is below the enforced floor.
    DelayBelowMinimum = 4,
    /// The supplied minimum delay exceeds the allowed maximum.
    DelayAboveMaximum = 5,
    /// The supplied role is not a recognized role.
    InvalidRole = 6,
    /// Attempted to rotate a role to the zero address.
    ZeroAddress = 7,
    /// The referenced proposal does not exist.
    ProposalNotFound = 8,
    /// The proposal has already been executed.
    ProposalAlreadyExecuted = 9,
    /// The proposal has already been cancelled.
    ProposalAlreadyCancelled = 10,
    /// The proposal's delay has not yet elapsed.
    DelayNotElapsed = 11,
    /// The proposal has already been queued.
    ProposalAlreadyQueued = 12,
    /// The predecessor proposal has not been executed yet.
    PredecessorNotExecuted = 13,
    /// The proposal is not in a state that permits this operation.
    InvalidProposalState = 14,
}
