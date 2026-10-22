use soroban_sdk::{contracttype, Address, BytesN, Symbol, Val, Vec};

/// Lifecycle state of a proposal.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
/// Lifecycle state of a proposal.
pub enum ProposalState {
    /// Voting has not started.
    Pending = 0,
    /// Voting is open.
    Active = 1,
    /// Canceled before execution.
    Canceled = 2,
    /// Voting ended without quorum or majority.
    Defeated = 3,
    /// Voting ended in favor; not yet queued.
    Succeeded = 4,
    /// Queued on the timelock.
    Queued = 5,
    /// Not executed before the grace period ended.
    Expired = 6,
    /// Executed.
    Executed = 7,
}

/// A governance proposal.
#[contracttype]
#[derive(Clone)]
pub struct Proposal {
    /// Account that created the proposal.
    pub proposer: Address,
    /// Contract called by each action.
    pub targets: Vec<Address>,
    /// Function called by each action.
    pub fns: Vec<Symbol>,
    /// Arguments of each action.
    pub args: Vec<Vec<Val>>,
    /// Voting power and quorum are read as of this ledger. Voting opens on
    /// the ledger after it.
    pub snapshot: u32,
    /// Last ledger on which votes are accepted.
    pub deadline: u32,
    /// Votes in favor.
    pub for_votes: i128,
    /// Votes against.
    pub against_votes: i128,
    /// Abstaining votes, which count toward quorum.
    pub abstain_votes: i128,
    /// Whether the proposal was canceled.
    pub canceled: bool,
    /// Timelock ready timestamp once queued, zero before.
    pub eta: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Token,
    Timelock,
    VotingDelay,
    VotingPeriod,
    QuorumBps,
    ProposalThreshold,
    Proposal(BytesN<32>),
    Voted(BytesN<32>, Address),
}
