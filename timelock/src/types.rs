use soroban_sdk::{contracttype, Address, BytesN, Symbol, Val, Vec};

/// One call in a scheduled batch.
#[contracttype]
#[derive(Clone)]
pub struct Call {
    /// The contract to call.
    pub target: Address,
    /// The function to call on it.
    pub function: Symbol,
    /// The arguments, in order.
    pub args: Vec<Val>,
}

/// The roles the timelock knows about.
///
/// A role is a *set* of addresses, not a single holder: the protocol hands
/// the timelock to one signer during the handover and to a council later,
/// without the contract having to be redeployed.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Role {
    /// May schedule operations.
    Proposer = 0,
    /// May execute a ready operation.
    Executor = 1,
    /// May cancel a pending operation.
    Canceller = 2,
}

/// A scheduled batch of calls.
///
/// The batch runs as one unit: `execute` walks the calls in order and a
/// revert in any of them reverts the whole batch. There is no partial
/// execution, so a batch either happened or did not.
#[contracttype]
#[derive(Clone)]
pub struct Operation {
    /// The calls, in order.
    pub calls: Vec<Call>,
    /// The id of the operation that must have run first, or the all-zero id
    /// when there is none.
    ///
    /// A contract type cannot hold an `Option<BytesN<32>>`, and an
    /// operation id is the SHA-256 of a payload, so the all-zero id is not
    /// one an operation can ever have: it is the "no predecessor" marker.
    pub predecessor: BytesN<32>,
    /// The salt that makes this operation's id unique.
    pub salt: BytesN<32>,
    /// The ledger timestamp from which the operation may be executed. The
    /// boundary is inclusive: an operation may run at exactly this moment.
    pub ready_at: u64,
    /// Set once the batch has run. A finished operation never runs again,
    /// which is what makes `execute` safe to retry after a crash.
    pub done: bool,
}

/// The all-zero operation id: the "no predecessor" marker.
///
/// A contract type cannot hold an `Option<BytesN<32>>`, and an operation id
/// is the SHA-256 of a payload, so the all-zero id is not one an operation
/// can ever have: it is safe to use as the marker.
pub fn is_no_predecessor(id: &BytesN<32>) -> bool {
    id.to_array() == [0u8; 32]
}

/// Storage keys.
///
/// The role and configuration keys live in instance storage; operations live
/// in persistent storage, because a scheduled batch outlives the
/// transaction that created it.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// The smallest delay an operation may be scheduled with.
    MinDelay,
    /// Whether anyone may execute a ready operation.
    OpenExecutor,
    /// Membership of one address in one role.
    Role(Role, Address),
    /// A scheduled operation, by id.
    Operation(BytesN<32>),
}
