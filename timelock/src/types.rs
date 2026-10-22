use soroban_sdk::{contracttype, Address, BytesN, Vec};

/// Current storage schema version for the timelock contract.
///
/// Bump this whenever the persistent layout of any `DataKey` variant changes
/// in a way that requires rewriting existing records. The `migrate` entrypoint
/// compares the stored version against this constant and walks old records
/// forward until they match.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Storage keys for the timelock contract.
///
/// The `SchemaVersion` variant lives in instance storage and is the single
/// source of truth for which layout the remaining keys are in. All other
/// variants are persistent so that role assignments and pending operations
/// survive ledger entry archival (subject to TTL extension).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Instance storage: schema version marker used by `migrate`.
    SchemaVersion,
    /// Persistent storage: address holding the admin role.
    Admin,
    /// Persistent storage: address holding the proposer role.
    Proposer,
    /// Persistent storage: address holding the executor role.
    Executor,
    /// Persistent storage: address holding the canceller role.
    Canceller,
    /// Persistent storage: minimum delay (in ledgers) before execution.
    MinDelay,
    /// Persistent storage: monotonically increasing operation id counter.
    OperationId,
    /// Persistent storage: pending/queued operation keyed by id.
    Operation(u64),
    /// Persistent storage: predecessor dependency for an operation id.
    Predecessor(u64),
    /// Persistent storage: timestamp at which an operation becomes ready.
    ReadyAt(u64),
    /// Persistent storage: cancellation flag for an operation id.
    Cancelled(u64),
    /// Persistent storage: set of operation ids awaiting execution.
    PendingOperations,
}

/// A queued timelock operation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Operation {
    pub target: Address,
    pub function: soroban_sdk::Symbol,
    pub args: Vec<soroban_sdk::Val>,
    pub eta: u64,
    pub predecessor: Option<u64>,
    pub salt: BytesN<32>,
}

/// Role identifiers used by the timelock access-control helpers.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Admin,
    Proposer,
    Executor,
    Canceller,
}
