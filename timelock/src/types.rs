use soroban_sdk::{contracttype, Address, BytesN, Symbol, Val, Vec};

#[contracttype]
#[derive(Clone)]
pub struct Operation {
    pub targets: Vec<Address>,
    pub fns: Vec<Symbol>,
    pub args: Vec<Vec<Val>>,
    /// Ledger timestamp from which the operation may be executed.
    pub ready_at: u64,
    pub done: bool,
    /// Operations that must have executed before this one can (empty, or
    /// the one passed to `schedule_after`). Always ones that already
    /// existed when this one was scheduled, so dependencies form a chain
    /// back in time and can never cycle.
    pub predecessors: Vec<BytesN<32>>,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// The only address that can schedule operations (the multisig during
    /// handover, the governor after).
    Proposer,
    /// Can cancel any pending operation and, until lock-in, reassign the
    /// proposer. Removed permanently by `lock_in`.
    Guardian,
    /// Extra addresses allowed to cancel pending operations (an emergency
    /// council, a monitoring key). Granted and revoked only through
    /// delayed `set_canc` self-calls.
    Canceller(Address),
    /// When set, only this address may call `execute`. Unset (the
    /// default) means anyone can execute a ready operation.
    Executor,
    MinDelay,
    /// How long after `ready_at` an operation stays executable.
    GracePeriod,
    Operation(BytesN<32>),
}
