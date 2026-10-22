use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    InvalidThreshold = 2,
    DuplicateSigner = 3,
    NotASigner = 4,
    AlreadyApproved = 5,
    NotYetApproved = 6,
    InvalidDelays = 7,
    AccountNotConfigured = 8,
    UnsortedSignatures = 9,
    InsufficientSignatures = 10,
    ContextNotAllowed = 11,
    ActionAlreadyRegistered = 12,
    ActionNotPending = 13,
    TooManyPendingActions = 14,
    ProposalNotFound = 15,
    ProposalTooLarge = 16,
    InvalidPageLimit = 17,
    NotExpired = 18,
    InvalidWeight = 19,
    WeightOverflow = 20,
    InvalidSignerChange = 21,
    SignerChangeNotFound = 22,
    StaleSignerChange = 23,
    RotationDelayActive = 24,
    SignerChangeNotQueued = 25,
    TooManySigners = 26,
    NotVetoer = 27,
    VetoerAlreadySet = 28,
    InvalidDeadline = 29,
    NotProposer = 30,
    DeadlineAlreadySet = 31,
    /// A state the contract guarantees by construction was missing (e.g. a
    /// storage entry that `initialize` always writes). Never expected on a
    /// correctly deployed contract; replaces a panic with a typed error.
    InternalInvariant = 90,
}
