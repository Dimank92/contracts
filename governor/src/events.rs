//! Events, schema v2 (docs/events.md): topics `(name, EVENTS_VERSION,
//! proposal id[, voter])`, data a named struct.
//!
//! `Pending` → `Active` → `Succeeded`/`Defeated` happen by ledger
//! progression, not by a call, so they have no event: an indexer derives
//! them from `snapshot`/`deadline`, the running tallies in `vote_cast`
//! and `quorum(snapshot)`. `Executed` is the timelock's own event.

use soroban_sdk::{contracttype, Address, BytesN, Env, Symbol, Val, Vec};
use zenith_common::events::{topics, EVENTS_VERSION};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializedEvent {
    pub token: Address,
    pub timelock: Address,
    pub voting_delay: u32,
    pub voting_period: u32,
    pub quorum_bps: u32,
    pub proposal_threshold: i128,
    pub schema_version: u32,
}

/// The whole proposal, so its actions are known without a read.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProposalCreatedEvent {
    pub proposer: Address,
    pub targets: Vec<Address>,
    pub fns: Vec<Symbol>,
    pub args: Vec<Vec<Val>>,
    pub description_hash: BytesN<32>,
    pub snapshot: u32,
    pub deadline: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoteCastEvent {
    pub support: u32,
    pub weight: i128,
    /// Tallies after this vote.
    pub for_votes: i128,
    pub against_votes: i128,
    pub abstain_votes: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalQueuedEvent {
    /// Timelock ready timestamp.
    pub eta: u64,
    /// Quorum at the snapshot, which the proposal met.
    pub quorum: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposalCanceledEvent {
    pub caller: Address,
    /// Whether a queued timelock operation was cancelled with it.
    pub was_queued: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParamSetEvent {
    pub value: i128,
}

fn keyed(env: &Env, name: &str, id: BytesN<32>) -> (Symbol, u32, BytesN<32>) {
    (Symbol::new(env, name), EVENTS_VERSION, id)
}

pub fn initialized(env: &Env, data: InitializedEvent) {
    env.events().publish(topics(env, "initialized"), data);
}

pub fn proposal_created(env: &Env, id: BytesN<32>, data: ProposalCreatedEvent) {
    env.events()
        .publish(keyed(env, "proposal_created", id), data);
}

pub fn vote_cast(env: &Env, id: BytesN<32>, voter: Address, data: VoteCastEvent) {
    env.events().publish(
        (Symbol::new(env, "vote_cast"), EVENTS_VERSION, id, voter),
        data,
    );
}

pub fn proposal_queued(env: &Env, id: BytesN<32>, data: ProposalQueuedEvent) {
    env.events()
        .publish(keyed(env, "proposal_queued", id), data);
}

pub fn proposal_canceled(env: &Env, id: BytesN<32>, data: ProposalCanceledEvent) {
    env.events()
        .publish(keyed(env, "proposal_canceled", id), data);
}

pub fn param_set(env: &Env, name: &str, value: i128) {
    env.events().publish(
        (
            Symbol::new(env, "param_set"),
            EVENTS_VERSION,
            Symbol::new(env, name),
        ),
        ParamSetEvent { value },
    );
}
