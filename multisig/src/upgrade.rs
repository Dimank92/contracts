//! Self-upgrade for the multisig, gated by its own, stricter threshold.
//!
//! The signer set, threshold and approval TTL are immutable by design, so
//! that the M signers who control day-to-day actions can't rewrite who
//! the signers are. A wasm upgrade could change all three, so letting M
//! signers upgrade would let them do indirectly what the design forbids
//! directly. The upgrade threshold is therefore **every** signer (N-of-N),
//! followed by `UPGRADE_DELAY`:
//!
//! 1. Each signer calls `approve_upgrade(signer, new_wasm_hash)`. Approvals
//!    are per hash, so consent is to one exact wasm.
//! 2. The approval that makes it unanimous starts the delay. Any signer
//!    can `revoke_upgrade` before execution, which stops the clock.
//! 3. After the delay, anyone may call `upgrade(new_wasm_hash)`.
//!
//! The ordinary approval flow (`approve` / `is_executable` / `execute`)
//! can't do this. `execute` calls its target as the multisig, and
//! Soroban forbids a contract calling itself. See docs/migrations.md,
//! "Governance decisions", for the liveness trade-off: one lost key
//! blocks upgrades for good.
//!
//! NOTE: lib.rs and types.rs are empty on main (emptied by #150). Once
//! they're restored, add `mod upgrade;` to lib.rs. This file only needs
//! `DataKey::Signers` (a `Vec<Address>`) and the `Multisig` struct.

use soroban_sdk::{
    contractimpl, contracttype, panic_with_error, Address, BytesN, Env, Symbol, Vec,
};

use crate::error::Error;
use crate::types::DataKey;
use crate::{ttl, Multisig};

/// Wait between the last signer's approval and `upgrade` becoming
/// callable, so users and integrators see a pending self-upgrade coming.
pub const UPGRADE_DELAY: u64 = 3 * 86_400;

#[contracttype]
#[derive(Clone)]
pub enum UpgradeKey {
    /// Present once `signer` has approved upgrading to the wasm hash.
    UpgradeApproval(BytesN<32>, Address),
    /// When the hash became unanimously approved. Cleared by any revoke.
    UpgradeUnanimousAt(BytesN<32>),
}

fn signers(env: &Env) -> Vec<Address> {
    env.storage().instance().get(&DataKey::Signers).unwrap_or_else(|| panic_with_error!(env, Error::InternalInvariant))
}

fn require_signer(env: &Env, signers: &Vec<Address>, signer: &Address) {
    signer.require_auth();
    if !signers.contains(signer) {
        panic_with_error!(env, Error::NotASigner);
    }
}

fn approvals(env: &Env, signers: &Vec<Address>, hash: &BytesN<32>) -> u32 {
    signers
        .iter()
        .filter(|s| {
            env.storage()
                .persistent()
                .has(&UpgradeKey::UpgradeApproval(hash.clone(), s.clone()))
        })
        .count() as u32
}

#[contractimpl]
impl Multisig {
    /// `signer` consents to upgrading this multisig to `new_wasm_hash`.
    /// Returns the approval count. The approval that makes it unanimous
    /// starts `UPGRADE_DELAY`.
    pub fn approve_upgrade(env: Env, signer: Address, new_wasm_hash: BytesN<32>) -> u32 {
        ttl::extend_instance(&env);
        let signers = signers(&env);
        require_signer(&env, &signers, &signer);
        let key = UpgradeKey::UpgradeApproval(new_wasm_hash.clone(), signer.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, Error::AlreadyApproved);
        }
        ttl::set_persistent(&env, &key, &true);

        let count = approvals(&env, &signers, &new_wasm_hash);
        if count == signers.len() {
            ttl::set_persistent(
                &env,
                &UpgradeKey::UpgradeUnanimousAt(new_wasm_hash.clone()),
                &env.ledger().timestamp(),
            );
        }
        env.events().publish(
            (Symbol::new(&env, "upgrade_approved"), new_wasm_hash),
            (signer, count),
        );
        count
    }

    /// Withdraws `signer`'s consent. Any revoke un-readies the upgrade;
    /// re-reaching unanimity restarts the delay.
    pub fn revoke_upgrade(env: Env, signer: Address, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        let signers = signers(&env);
        require_signer(&env, &signers, &signer);
        let key = UpgradeKey::UpgradeApproval(new_wasm_hash.clone(), signer.clone());
        if !env.storage().persistent().has(&key) {
            panic_with_error!(&env, Error::NotYetApproved);
        }
        env.storage().persistent().remove(&key);
        env.storage()
            .persistent()
            .remove(&UpgradeKey::UpgradeUnanimousAt(new_wasm_hash.clone()));
        env.events().publish(
            (Symbol::new(&env, "upgrade_revoked"), new_wasm_hash),
            signer,
        );
    }

    /// Anyone, once every signer has approved `new_wasm_hash` and
    /// `UPGRADE_DELAY` has passed since. Clears the approvals, then swaps
    /// the wasm. Follow with `migrate` if the new wasm raises the schema
    /// version.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        let signers = signers(&env);
        let at_key = UpgradeKey::UpgradeUnanimousAt(new_wasm_hash.clone());
        let unanimous_at: u64 = ttl::get_persistent(&env, &at_key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::UpgradeNotUnanimous));
        // Defensive re-count: UnanimousAt is only ever set at N-of-N and
        // cleared by any revoke, but the signer set is what's being
        // protected, so check it directly.
        if approvals(&env, &signers, &new_wasm_hash) != signers.len() {
            panic_with_error!(&env, Error::UpgradeNotUnanimous);
        }
        if env.ledger().timestamp() < unanimous_at.saturating_add(UPGRADE_DELAY) {
            panic_with_error!(&env, Error::UpgradeDelayNotElapsed);
        }

        env.storage().persistent().remove(&at_key);
        for signer in signers.iter() {
            env.storage()
                .persistent()
                .remove(&UpgradeKey::UpgradeApproval(new_wasm_hash.clone(), signer));
        }
        env.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());
        env.events()
            .publish((Symbol::new(&env, "upgraded"),), new_wasm_hash);
    }

    pub fn get_upgrade_approvals(env: Env, new_wasm_hash: BytesN<32>) -> u32 {
        ttl::extend_instance(&env);
        approvals(&env, &signers(&env), &new_wasm_hash)
    }

    /// When `upgrade(new_wasm_hash)` becomes callable, if every signer
    /// has approved it.
    pub fn get_upgrade_ready_at(env: Env, new_wasm_hash: BytesN<32>) -> Option<u64> {
        ttl::extend_instance(&env);
        let at: Option<u64> =
            ttl::get_persistent(&env, &UpgradeKey::UpgradeUnanimousAt(new_wasm_hash));
        at.map(|t| t.saturating_add(UPGRADE_DELAY))
    }
}
