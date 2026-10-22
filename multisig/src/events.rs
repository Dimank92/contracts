use soroban_sdk::{Address, BytesN, Env, Symbol};

use crate::types::SignerChange;

pub fn registered(
    env: &Env,
    proposer: Address,
    action_id: BytesN<32>,
    description_hash: BytesN<32>,
) {
    env.events().publish(
        (Symbol::new(env, "registered"), proposer, action_id),
        description_hash,
    );
}

pub fn approved(env: &Env, signer: Address, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "approved"), signer, action_id),
        description_hash,
    );
}

pub fn revoked(env: &Env, signer: Address, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "revoked"), signer, action_id),
        description_hash,
    );
}

pub fn reset(env: &Env, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "reset"), action_id), description_hash);
}

pub fn expired(env: &Env, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "expired"), action_id), description_hash);
}

pub fn executed(env: &Env, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "executed"), action_id), description_hash);
}

/// A single signer's weight alone meets the approval threshold.
pub fn single_key_quorum(env: &Env, signer: Address, weight: u32, threshold: u32) {
    env.events().publish(
        (Symbol::new(env, "single_key_quorum"), signer),
        (weight, threshold),
    );
}

pub fn signer_change_proposed(
    env: &Env,
    proposer: Address,
    change_id: BytesN<32>,
    change: SignerChange,
) {
    env.events().publish(
        (
            Symbol::new(env, "signer_change_proposed"),
            proposer,
            change_id,
        ),
        change,
    );
}

pub fn signer_change_queued(env: &Env, change_id: BytesN<32>, ready_at: u64) {
    env.events().publish(
        (Symbol::new(env, "signer_change_queued"), change_id),
        ready_at,
    );
}

pub fn signer_change_vetoed(env: &Env, signer: Address, change_id: BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "signer_change_vetoed"), signer),
        change_id,
    );
}

pub fn signers_rotated(env: &Env, change_id: BytesN<32>, epoch: u32) {
    env.events()
        .publish((Symbol::new(env, "signers_rotated"), change_id), epoch);
}

pub fn vetoed(env: &Env, vetoer: Address, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "vetoed"), vetoer, action_id),
        description_hash,
    );
}

pub fn action_expired(env: &Env, action_id: BytesN<32>, execute_by: u64) {
    env.events()
        .publish((Symbol::new(env, "action_expired"), action_id), execute_by);
}

pub fn cancelled(env: &Env, signer: Address, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "cancelled"), signer, action_id),
        description_hash,
    );
}
