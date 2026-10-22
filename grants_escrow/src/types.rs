use soroban_sdk::{contracttype, Address, BytesN, Vec};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MilestoneStatus {
    Pending,   // funded, awaiting the grantee's submission
    Submitted, // evidence submitted, awaiting reviewer quorum
    Released,  // quorum reached, paid to the grantee
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

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    pub amount: i128,
    pub deadline: u64,
    pub status: MilestoneStatus,
    /// All zeroes until the grantee submits.
    pub evidence_hash: BytesN<32>,
    pub approvals: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Grant {
    pub grant_id: u64,
    pub funder: Address,
    pub grantee: Address,
    pub token: Address,
    pub reviewers: Vec<Address>,
    pub quorum: u32,
    pub milestones: Vec<Milestone>,
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
