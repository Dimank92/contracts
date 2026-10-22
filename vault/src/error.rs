use soroban_sdk::contracterror;

/// Errors returned by the vault contract.
///
/// Tag validation errors (see issue #204) are grouped at the end so existing
/// discriminants stay stable for already-deployed callers.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum VaultError {
    /// The vault has not been initialized.
    NotInitialized = 1,
    /// The vault has already been initialized.
    AlreadyInitialized = 2,
    /// Caller is not authorized for this operation.
    Unauthorized = 3,
    /// Requested amount exceeds the balance held under the tag.
    InsufficientBalance = 4,
    /// Amount must be strictly positive.
    InvalidAmount = 5,
    /// The integrator is not on the allowlist.
    IntegratorNotAllowed = 6,
    /// The integrator is already on the allowlist.
    IntegratorAlreadyAllowed = 7,
    /// The vault is paused and cannot service this operation.
    Paused = 8,
    /// A tag component is empty.
    EmptyTagComponent = 9,
    /// A tag component exceeds the maximum allowed length.
    TagComponentTooLong = 10,
    /// The tag does not match the documented `(integrator, scope, id)` layout.
    InvalidTagShape = 11,
}
