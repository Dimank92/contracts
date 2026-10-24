//! Events, schema v2 (docs/events.md): topics `(name, EVENTS_VERSION,
//! id[, signer])`, data a named struct. Approval events carry the running
//! tally, and rotation events the resulting signer set, so an indexer can
//! mirror the multisig without reads.
//!
//! `prune` emits nothing: it only deletes approval entries that can no
//! longer affect `is_approved`.

use soroban_sdk::{contracttype, Address, BytesN, Env, Map, Symbol, Val, Vec};
use zenith_common::events::{topics, EVENTS_VERSION};

use crate::types::SignerChange;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializedEvent {
    /// Every signer and their weight.
    pub signers: Map<Address, u32>,
    pub threshold: u32,
    pub approval_ttl: u64,
    pub rotation_threshold: u32,
    pub rotation_delay: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredEvent {
    pub proposer: Address,
    pub description_hash: BytesN<32>,
    pub created_at: u64,
}

/// The call an executor proposal will make (its id is also registered).
#[contracttype]
#[derive(Clone, Debug)]
pub struct ProposalCreatedEvent {
    pub target: Address,
    pub function: Symbol,
    pub args: Vec<Val>,
    pub nonce: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovedEvent {
    pub description_hash: BytesN<32>,
    pub approved_at: u64,
    /// Fresh approvals after this one.
    pub approval_count: u32,
    pub approval_weight: u32,
    /// Signer-set epoch the approval counts in.
    pub epoch: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevokedEvent {
    pub description_hash: BytesN<32>,
    pub approval_count: u32,
    pub approval_weight: u32,
}

/// Data of `reset`, `expired` and `executed`: the action moved to that
/// final status and its approvals were cleared.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionFinishedEvent {
    pub description_hash: BytesN<32>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionExpiredEvent {
    pub description_hash: BytesN<32>,
    pub execute_by: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VetoedEvent {
    pub vetoer: Address,
    pub description_hash: BytesN<32>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VetoerSetEvent {
    pub vetoer: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecuteBySetEvent {
    pub execute_by: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SingleKeyQuorumEvent {
    pub weight: u32,
    pub threshold: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignerChangeProposedEvent {
    pub proposer: Address,
    pub change: SignerChange,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignerChangeQueuedEvent {
    pub ready_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignerChangeVetoedEvent {
    pub signer: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignersRotatedEvent {
    /// The new signer-set epoch; every earlier approval is void.
    pub epoch: u32,
    pub signers: Map<Address, u32>,
    pub threshold: u32,
    pub rotation_threshold: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeApprovedEvent {
    pub signer: Address,
    pub approvals: u32,
    pub signer_count: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeRevokedEvent {
    pub signer: Address,
    pub approvals: u32,
}

fn keyed(env: &Env, name: &str, id: BytesN<32>) -> (Symbol, u32, BytesN<32>) {
    (Symbol::new(env, name), EVENTS_VERSION, id)
}

pub fn initialized(env: &Env, data: InitializedEvent) {
    env.events().publish(topics(env, "initialized"), data);
}

pub fn registered(env: &Env, action_id: BytesN<32>, data: RegisteredEvent) {
    env.events()
        .publish(keyed(env, "registered", action_id), data);
}

pub fn proposal_created(env: &Env, proposal_id: BytesN<32>, data: ProposalCreatedEvent) {
    env.events()
        .publish(keyed(env, "proposal_created", proposal_id), data);
}

pub fn approved(env: &Env, action_id: BytesN<32>, signer: Address, data: ApprovedEvent) {
    env.events().publish(
        (
            Symbol::new(env, "approved"),
            EVENTS_VERSION,
            action_id,
            signer,
        ),
        data,
    );
}

pub fn revoked(env: &Env, action_id: BytesN<32>, signer: Address, data: RevokedEvent) {
    env.events().publish(
        (
            Symbol::new(env, "revoked"),
            EVENTS_VERSION,
            action_id,
            signer,
        ),
        data,
    );
}

fn finished(env: &Env, name: &str, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events().publish(
        keyed(env, name, action_id),
        ActionFinishedEvent { description_hash },
    );
}

pub fn reset(env: &Env, action_id: BytesN<32>, description_hash: BytesN<32>) {
    finished(env, "reset", action_id, description_hash);
}

pub fn expired(env: &Env, action_id: BytesN<32>, description_hash: BytesN<32>) {
    finished(env, "expired", action_id, description_hash);
}

pub fn executed(env: &Env, action_id: BytesN<32>, description_hash: BytesN<32>) {
    finished(env, "executed", action_id, description_hash);
}

pub fn action_expired(env: &Env, action_id: BytesN<32>, data: ActionExpiredEvent) {
    env.events()
        .publish(keyed(env, "action_expired", action_id), data);
}

pub fn vetoed(env: &Env, action_id: BytesN<32>, data: VetoedEvent) {
    env.events().publish(keyed(env, "vetoed", action_id), data);
}

pub fn vetoer_set(env: &Env, data: VetoerSetEvent) {
    env.events().publish(topics(env, "vetoer_set"), data);
}

pub fn execute_by_set(env: &Env, action_id: BytesN<32>, data: ExecuteBySetEvent) {
    env.events()
        .publish(keyed(env, "execute_by_set", action_id), data);
}

/// A single signer's weight alone meets the approval threshold.
pub fn single_key_quorum(env: &Env, signer: Address, weight: u32, threshold: u32) {
    env.events().publish(
        (
            Symbol::new(env, "single_key_quorum"),
            EVENTS_VERSION,
            signer,
        ),
        SingleKeyQuorumEvent { weight, threshold },
    );
}

pub fn signer_change_proposed(env: &Env, change_id: BytesN<32>, data: SignerChangeProposedEvent) {
    env.events()
        .publish(keyed(env, "signer_change_proposed", change_id), data);
}

pub fn signer_change_queued(env: &Env, change_id: BytesN<32>, ready_at: u64) {
    env.events().publish(
        keyed(env, "signer_change_queued", change_id),
        SignerChangeQueuedEvent { ready_at },
    );
}

pub fn signer_change_vetoed(env: &Env, change_id: BytesN<32>, signer: Address) {
    env.events().publish(
        keyed(env, "signer_change_vetoed", change_id),
        SignerChangeVetoedEvent { signer },
    );
}

pub fn signers_rotated(env: &Env, change_id: BytesN<32>, data: SignersRotatedEvent) {
    env.events()
        .publish(keyed(env, "signers_rotated", change_id), data);
}

pub fn upgrade_approved(env: &Env, wasm_hash: BytesN<32>, data: UpgradeApprovedEvent) {
    env.events()
        .publish(keyed(env, "upgrade_approved", wasm_hash), data);
}

pub fn upgrade_revoked(env: &Env, wasm_hash: BytesN<32>, data: UpgradeRevokedEvent) {
    env.events()
        .publish(keyed(env, "upgrade_revoked", wasm_hash), data);
}

pub fn cancelled(env: &Env, signer: Address, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "cancelled"), signer, action_id),
        description_hash,
    );
}
