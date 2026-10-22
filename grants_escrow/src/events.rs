use soroban_sdk::{Address, BytesN, Env, Symbol};

use crate::types::DisputeOutcome;

pub fn grant_created(env: &Env, grant_id: u64, funder: Address, grantee: Address, total: i128) {
    env.events().publish(
        (Symbol::new(env, "grant_created"), grant_id),
        (funder, grantee, total),
    );
}

pub fn milestone_submitted(env: &Env, grant_id: u64, idx: u32, evidence_hash: BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "milestone_submitted"), grant_id, idx),
        evidence_hash,
    );
}

pub fn milestone_approved(env: &Env, grant_id: u64, idx: u32, reviewer: Address, approvals: u32) {
    env.events().publish(
        (Symbol::new(env, "milestone_approved"), grant_id, idx),
        (reviewer, approvals),
    );
}

pub fn milestone_released(env: &Env, grant_id: u64, idx: u32, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "milestone_released"), grant_id, idx),
        amount,
    );
}

pub fn funds_reclaimed(env: &Env, grant_id: u64, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "funds_reclaimed"), grant_id), amount);
}

pub fn dispute_opened(env: &Env, grant_id: u64, idx: u32, opened_by: Address) {
    env.events().publish(
        (Symbol::new(env, "dispute_opened"), grant_id, idx),
        opened_by,
    );
}

pub fn dispute_resolved(
    env: &Env,
    grant_id: u64,
    idx: u32,
    outcome: DisputeOutcome,
    to_grantee: i128,
    to_funder: i128,
) {
    env.events().publish(
        (Symbol::new(env, "dispute_resolved"), grant_id, idx),
        (outcome, to_grantee, to_funder),
    );
}

pub fn refunded(env: &Env, grant_id: u64, idx: u32, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "refunded"), grant_id, idx), amount);
}
