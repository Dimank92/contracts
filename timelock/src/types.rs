use soroban_sdk::{contracttype, Address, BytesN, Symbol, Val, Vec};

/// Longest predecessor chain `schedule` and `execute` will walk (in nodes). Bounds
/// the ledger reads per call; deeper chains are rejected, not truncated.
pub const MAX_CHAIN_DEPTH: u32 = 8;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Call {
    pub target: Address,
    pub function: Symbol,
    pub args: Vec<Val>,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Role {
    Proposer,
    Executor,
    Canceller,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Status {
    Pending,
    Done,
    /// Kept as a tombstone so dependents can tell "cancelled" from "unknown".
    Cancelled,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Operation {
    pub status: Status,
    pub ready_at: u64,
    /// Zero or one id (`Option<BytesN<32>>` doesn't derive `contracttype`
    /// with testutils on in this SDK).
    pub predecessor: Vec<BytesN<32>>,
}

#[contracttype]
pub enum DataKey {
    MinDelay,
    OpenExecutor,
    Role(Role, Address),
    Op(BytesN<32>),
}
