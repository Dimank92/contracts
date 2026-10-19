//! Self-upgrade for the multisig, gated by its own, stricter threshold
//! and the shared upgrade-safety framework (zenith_common::upgrade,
//! docs/upgrades.md).
//!
//! The signer set, threshold and approval TTL can only change through
//! the delayed, vetoable rotation flow, so that the M signers who control
//! day-to-day actions can't rewrite who the signers are. A wasm upgrade
//! could change all three, so letting M signers upgrade would let them do
//! indirectly what the design forbids directly. The upgrade threshold is
//! therefore **every** signer (N-of-N), followed by `UPGRADE_DELAY`:
//!
//! 1. Each signer calls `approve_upgrade(signer, new_wasm_hash)`. Approvals
//!    are per hash, so consent is to one exact wasm.
//! 2. The approval that makes it unanimous allowlists the hash with
//!    `not_before = now + UPGRADE_DELAY` (`upgrade_proposed`). Any signer
//!    can `revoke_upgrade` before installation, which removes it from the
//!    allowlist (`upgrade_cancelled`); re-reaching unanimity restarts the
//!    delay.
//! 3. After the delay, anyone may call `upgrade(new_wasm_hash)`, which
//!    re-checks unanimity against the *current* signer set, installs the
//!    wasm and raises the post-upgrade guard.
//! 4. `migrate` (if the new wasm raises the schema), then anyone calls
//!    `post_upgrade_check()` on the new code. Until it passes, every
//!    state-changing entrypoint fails with `UpgradeCheckPending`.
//!
//! **No emergency path.** N-of-N is already the highest threshold the
//! multisig has, so there is nothing higher to gate a shorter delay on.
//! A botched upgrade is fixed by unanimously approving a fixed wasm,
//! which waits the full delay; meanwhile the guard keeps the multisig
//! closed.
//!
//! The ordinary approval flow (`approve` / `is_approved` / `execute`)
//! can't do this. `execute` calls its target as the multisig, and
//! Soroban forbids a contract calling itself. See docs/migrations.md,
//! "Governance decisions", for the liveness trade-off: one lost key
//! blocks upgrades for good.

use soroban_sdk::{contractimpl, contracttype, Address, BytesN, Env, Map};
use zenith_common::upgrade::{self, UpgradeErrors};

use crate::error::Error;
use crate::events::{self, UpgradeApprovedEvent, UpgradeRevokedEvent};
use crate::migrations::CURRENT_SCHEMA_VERSION;
use crate::types::{DataKey, UpgradeStatus};
use crate::{ttl, Multisig, MultisigClient};

/// Wait between the last signer's approval and `upgrade` becoming
/// callable, so users and integrators see a pending self-upgrade coming.
pub const UPGRADE_DELAY: u64 = upgrade::UPGRADE_DELAY;

pub const UPGRADE_ERRORS: UpgradeErrors<Error> = UpgradeErrors {
    unauthorized: Error::NotASigner,
    not_initialized: Error::NotInitialized,
    not_proposed: Error::WasmNotProposed,
    not_matured: Error::UpgradeNotMatured,
    already_proposed: Error::WasmAlreadyProposed,
    check_pending: Error::UpgradeCheckPending,
    no_check_pending: Error::NoUpgradeCheckPending,
    migration_in_progress: Error::MigrationInProgress,
};

#[contracttype]
#[derive(Clone)]
pub enum UpgradeKey {
    /// Present once `signer` has approved upgrading to the wasm hash.
    UpgradeApproval(BytesN<32>, Address),
}

fn signers(env: &Env) -> Result<Map<Address, u32>, Error> {
    env.storage()
        .instance()
        .get(&DataKey::Signers)
        .ok_or(Error::NotInitialized)
}

fn require_signer(signers: &Map<Address, u32>, signer: &Address) -> Result<(), Error> {
    signer.require_auth();
    if !signers.contains_key(signer.clone()) {
        return Err(Error::NotASigner);
    }
    Ok(())
}

fn approvals(env: &Env, signers: &Map<Address, u32>, hash: &BytesN<32>) -> u32 {
    signers
        .keys()
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
    /// allowlists the hash, installable after `UPGRADE_DELAY`.
    pub fn approve_upgrade(
        env: Env,
        signer: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        let signers = signers(&env)?;
        require_signer(&signers, &signer)?;
        let key = UpgradeKey::UpgradeApproval(new_wasm_hash.clone(), signer.clone());
        if env.storage().persistent().has(&key) {
            return Err(Error::AlreadyApproved);
        }
        ttl::set_persistent(&env, &key, &true);

        let count = approvals(&env, &signers, &new_wasm_hash);
        events::upgrade_approved(
            &env,
            new_wasm_hash.clone(),
            UpgradeApprovedEvent {
                signer,
                approvals: count,
                signer_count: signers.len(),
            },
        );
        if count == signers.len() {
            // Re-reaching unanimity (e.g. after a signer was added)
            // restarts the delay.
            if upgrade::proposed_wasm(&env, &new_wasm_hash).is_some() {
                upgrade::cancel_wasm(&env, &new_wasm_hash, &UPGRADE_ERRORS)?;
            }
            upgrade::propose_wasm(&env, &new_wasm_hash, UPGRADE_DELAY, false, &UPGRADE_ERRORS)?;
        }
        Ok(count)
    }

    /// Withdraws `signer`'s consent. Any revoke removes the hash from the
    /// allowlist; re-reaching unanimity restarts the delay.
    pub fn revoke_upgrade(
        env: Env,
        signer: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), Error> {
        ttl::extend_instance(&env);
        let signers = signers(&env)?;
        require_signer(&signers, &signer)?;
        let key = UpgradeKey::UpgradeApproval(new_wasm_hash.clone(), signer.clone());
        if !env.storage().persistent().has(&key) {
            return Err(Error::NotYetApproved);
        }
        env.storage().persistent().remove(&key);
        if upgrade::proposed_wasm(&env, &new_wasm_hash).is_some() {
            upgrade::cancel_wasm(&env, &new_wasm_hash, &UPGRADE_ERRORS)?;
        }
        events::upgrade_revoked(
            &env,
            new_wasm_hash.clone(),
            UpgradeRevokedEvent {
                signer,
                approvals: approvals(&env, &signers, &new_wasm_hash),
            },
        );
        Ok(())
    }

    /// Anyone, once every current signer has approved `new_wasm_hash`
    /// and its allowlist entry has matured. Clears the approvals, raises
    /// the post-upgrade guard, then swaps the wasm. Follow with `migrate`
    /// if the new wasm raises the schema version, then
    /// `post_upgrade_check`.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), Error> {
        ttl::extend_instance(&env);
        let signers = signers(&env)?;
        // The allowlist entry only exists while unanimity held, but the
        // signer set is what's being protected, so check it directly.
        if approvals(&env, &signers, &new_wasm_hash) != signers.len() {
            return Err(Error::UpgradeNotUnanimous);
        }
        for signer in signers.keys().iter() {
            env.storage()
                .persistent()
                .remove(&UpgradeKey::UpgradeApproval(new_wasm_hash.clone(), signer));
        }
        upgrade::install(&env, &new_wasm_hash, &UPGRADE_ERRORS)
    }

    /// Permissionless self-test on newly installed code: requires the
    /// schema to be current, then re-validates the signer set and
    /// thresholds. `true` lowers the guard; `false` records the failure
    /// (see `get_upgrade_status`) and keeps the multisig closed.
    pub fn post_upgrade_check(env: Env) -> Result<bool, Error> {
        ttl::extend_instance(&env);
        upgrade::post_upgrade_check(
            &env,
            CURRENT_SCHEMA_VERSION,
            Multisig::check_invariants,
            &UPGRADE_ERRORS,
        )
    }

    /// The post-upgrade guard, if an installed upgrade hasn't passed its
    /// self-check yet.
    pub fn get_upgrade_status(env: Env) -> Result<Option<UpgradeStatus>, Error> {
        ttl::extend_instance(&env);
        Ok(upgrade::pending_check(&env).map(|p| UpgradeStatus {
            wasm_hash: p.wasm_hash,
            installed_at: p.installed_at,
            failed: p.failed,
            reason: p.reason,
        }))
    }

    pub fn get_upgrade_approvals(env: Env, new_wasm_hash: BytesN<32>) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        Ok(approvals(&env, &signers(&env)?, &new_wasm_hash))
    }

    /// When `upgrade(new_wasm_hash)` becomes callable, if every signer
    /// has approved it (it's on the allowlist).
    pub fn get_upgrade_ready_at(env: Env, new_wasm_hash: BytesN<32>) -> Result<Option<u64>, Error> {
        ttl::extend_instance(&env);
        Ok(upgrade::proposed_wasm(&env, &new_wasm_hash).map(|p| p.not_before))
    }
}
