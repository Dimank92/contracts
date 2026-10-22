use soroban_sdk::{contracttype, Address};

/// Role holders for the timelock. Each role is stored separately so that
/// rotation of one holder does not disturb the others.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Role {
    Proposer,
    Executor,
    Canceller,
    Guardian,
}

/// A queued proposal. `eta` is captured at queue time so that later changes
/// to the minimum delay never retroactively shorten (or lengthen) the exit
/// window of an already-queued proposal.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub id: u64,
    pub proposer: Address,
    pub target: Address,
    pub function: soroban_sdk::Symbol,
    pub args: soroban_sdk::Bytes,
    pub predecessor: Option<u64>,
    pub queued_at: u64,
    pub eta: u64,
    pub executed: bool,
    pub cancelled: bool,
}

/// Storage keys for the timelock contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    MinDelay,
    Proposer,
    Executor,
    Canceller,
    Guardian,
    NextProposalId,
    Proposal(u64),
}
