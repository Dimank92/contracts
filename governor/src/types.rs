use soroban_sdk::{contracttype, Address, BytesN, Symbol, Val, Vec};

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ProposalState {
    Pending = 0,
    Active = 1,
    Canceled = 2,
    Defeated = 3,
    Succeeded = 4,
    Queued = 5,
    Expired = 6,
    Executed = 7,
}

#[contracttype]
#[derive(Clone)]
pub struct Proposal {
    pub proposer: Address,
    pub targets: Vec<Address>,
    pub fns: Vec<Symbol>,
    pub args: Vec<Vec<Val>>,
    /// Voting power and quorum are read as of this ledger. Voting opens on
    /// the ledger after it.
    pub snapshot: u32,
    /// Last ledger on which votes are accepted.
    pub deadline: u32,
    pub for_votes: i128,
    pub against_votes: i128,
    pub abstain_votes: i128,
    pub canceled: bool,
    /// Timelock ready timestamp once queued, zero before.
    pub eta: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// One-shot initialize flag; see `zenith_common::require_uninitialized`.
    Initialized,
    Token,
    Timelock,
    VotingDelay,
    VotingPeriod,
    QuorumBps,
    ProposalThreshold,
    Proposal(BytesN<32>),
    Voted(BytesN<32>, Address),
}
