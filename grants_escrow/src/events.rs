//! Events, schema v2 (docs/events.md): topics `(name, EVENTS_VERSION,
//! grant_id[, milestone index])`, data a named struct.

use soroban_sdk::{contracttype, Address, BytesN, Env, Symbol, Vec};
use zenith_common::events::EVENTS_VERSION;

/// Everything about a new grant. Every milestone starts `Pending` with no
/// approvals.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrantCreatedEvent {
    pub funder: Address,
    pub grantee: Address,
    pub token: Address,
    /// Sum of every milestone amount, pulled from the funder.
    pub total: i128,
    pub reviewers: Vec<Address>,
    pub quorum: u32,
    /// Milestone amounts, by index.
    pub amounts: Vec<i128>,
    /// Milestone deadlines, by index.
    pub deadlines: Vec<u64>,
    pub created_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneSubmittedEvent {
    pub evidence_hash: BytesN<32>,
    pub submitted_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneApprovedEvent {
    pub reviewer: Address,
    /// Approvals so far, including this one.
    pub approvals: u32,
    pub quorum: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneReleasedEvent {
    pub grantee: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FundsReclaimedEvent {
    pub funder: Address,
    pub amount: i128,
    /// Indexes of the milestones that moved to `Reclaimed`.
    pub milestones: Vec<u32>,
}

pub fn grant_created(env: &Env, grant_id: u64, data: GrantCreatedEvent) {
    env.events().publish(
        (Symbol::new(env, "grant_created"), EVENTS_VERSION, grant_id),
        data,
    );
}

fn milestone_topics(env: &Env, name: &str, grant_id: u64, idx: u32) -> (Symbol, u32, u64, u32) {
    (Symbol::new(env, name), EVENTS_VERSION, grant_id, idx)
}

pub fn milestone_submitted(env: &Env, grant_id: u64, idx: u32, data: MilestoneSubmittedEvent) {
    env.events().publish(
        milestone_topics(env, "milestone_submitted", grant_id, idx),
        data,
    );
}

pub fn milestone_approved(env: &Env, grant_id: u64, idx: u32, data: MilestoneApprovedEvent) {
    env.events().publish(
        milestone_topics(env, "milestone_approved", grant_id, idx),
        data,
    );
}

pub fn milestone_released(env: &Env, grant_id: u64, idx: u32, data: MilestoneReleasedEvent) {
    env.events().publish(
        milestone_topics(env, "milestone_released", grant_id, idx),
        data,
    );
}

pub fn funds_reclaimed(env: &Env, grant_id: u64, data: FundsReclaimedEvent) {
    env.events().publish(
        (
            Symbol::new(env, "funds_reclaimed"),
            EVENTS_VERSION,
            grant_id,
        ),
        data,
    );
}
