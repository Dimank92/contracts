use soroban_sdk::{contracttype, Address, BytesN, Vec};

/// Progress of a single milestone.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MilestoneStatus {
    /// Funded, awaiting the grantee's submission.
    Pending,   // funded, awaiting the grantee's submission
    /// Evidence submitted, awaiting reviewer quorum.
    Submitted, // evidence submitted, awaiting reviewer quorum
    /// Quorum reached, paid to the grantee.
    Released,  // quorum reached, paid to the grantee
    /// Deadline and grace missed, returned to the funder.
    Reclaimed, // deadline + grace missed, returned to the funder
    Disputed,  // frozen: funder or grantee opened a dispute, awaiting the arbiter
    Refunded,  // dispute rejected, returned to the funder
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisputeOutcome {
    Release, // pay the whole milestone to the grantee
    Reject,  // refund the whole milestone to the funder
    Split,   // `split_bps` to the grantee, the remainder to the funder
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dispute {
    pub opened_by: Address,
    pub opened_at: u64,
    pub resolved: bool,
    /// Only meaningful once `resolved`.
    pub outcome: DisputeOutcome,
    /// Basis points paid to the grantee; meaningful for `Split`.
    pub split_bps: u32,
}

/// One payable step of a grant.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    /// Escrowed amount for this milestone.
    pub amount: i128,
    /// Submission deadline (ledger timestamp).
    pub deadline: u64,
    /// Current status.
    pub status: MilestoneStatus,
    /// All zeroes until the grantee submits.
    pub evidence_hash: BytesN<32>,
    /// Reviewer approvals collected so far.
    pub approvals: u32,
}

/// A funded grant.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Grant {
    /// Unique grant id.
    pub grant_id: u64,
    /// Account that funded the escrow.
    pub funder: Address,
    /// Account receiving released milestones.
    pub grantee: Address,
    /// Escrowed token.
    pub token: Address,
    /// Accounts allowed to approve milestones.
    pub reviewers: Vec<Address>,
    /// Approvals needed to release a milestone.
    pub quorum: u32,
    /// Milestones in payout order.
    pub milestones: Vec<Milestone>,
    /// Creation timestamp.
    pub created_at: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    GrantCounter,
    Grant(u64),
    /// Present once `reviewer` has approved milestone `idx` of a grant.
    Approval(u64, u32, Address),
    /// Every grant id ever created for a grantee, in creation order.
    GranteeGrants(Address),
    /// Instance: the dispute arbiter, set once by `initialize`.
    Arbiter,
    /// Dispute record for milestone `idx` of a grant.
    Dispute(u64, u32),
}
