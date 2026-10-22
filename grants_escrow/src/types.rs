use soroban_sdk::{contracttype, Address, BytesN, Vec};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MilestoneStatus {
    Pending,   // funded, awaiting the grantee's submission
    Submitted, // evidence submitted, awaiting reviewer quorum
    Released,  // quorum reached, paid to the grantee
    Reclaimed, // deadline + grace missed, returned to the funder
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
    /// Total amount funded into escrow at creation time.
    pub total_amount: i128,
    /// Amount still held in escrow (funded minus released minus reclaimed).
    pub remaining: i128,
    /// Whether the grant has been cancelled; once true no further claims.
    pub cancelled: bool,
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
    /// Admin address used by `common::require_admin_or_multisig`.
    Admin,
    /// Vault tag under which a grant's escrow is accounted.
    /// Derived from grant_id but stored explicitly so it can be looked up
    /// without recomputing the tag encoding.
    GrantTag(u64),
}