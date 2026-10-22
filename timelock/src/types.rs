use soroban_sdk::{contracttype, Address, Symbol, Val, Vec};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Proposer,
    Executor,
    Canceller,
    Delay,
    /// Multisig/guardian wired as an additional canceller, if any.
    Guardian,
    /// Next proposal id; also the number of proposals ever created.
    NextId,
    /// Ids of proposals still `Pending`, in ascending order.
    Pending,
    Proposal(u64),
}

/// The delayed action: `target.func(args)`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Call {
    pub target: Address,
    pub func: Symbol,
    pub args: Vec<Val>,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ProposalStatus {
    Pending = 0,
    Executed = 1,
    Cancelled = 2,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Proposal {
    pub id: u64,
    pub call: Call,
    /// Must be `Executed` before this proposal can execute.
    pub predecessor: Option<u64>,
    pub proposer: Address,
    /// Ledger timestamp (seconds) from which the proposal may execute.
    pub execute_after: u64,
    pub status: ProposalStatus,
}
