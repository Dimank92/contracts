use soroban_sdk::{Address, BytesN, Env, Symbol};

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

pub fn claim(env: &Env, grant_id: u64, idx: u32, grantee: Address, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "claim"), grant_id, idx),
        (grantee, amount),
    );
}

pub fn grant_cancelled(env: &Env, grant_id: u64, grantor: Address, refunded: i128) {
    env.events().publish(
        (Symbol::new(env, "grant_cancelled"), grant_id),
        (grantor, refunded),
    );
}

pub fn cancel_grant(env: &Env, grant_id: u64, grantor: Address, refunded: i128) {
    env.events().publish(
        (Symbol::new(env, "cancel_grant"), grant_id),
        (grantor, refunded),
    );
}

pub fn funds_reclaimed(env: &Env, grant_id: u64, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "funds_reclaimed"), grant_id), amount);
}