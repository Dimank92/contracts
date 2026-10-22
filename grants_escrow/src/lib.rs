// Public API docs are enforced by `cargo xtask doc` (#268): the gate builds
// rustdoc, where this lint is on, and only there — soroban-sdk macro output is
// undocumented, which would otherwise fail `clippy -D warnings`.
#![cfg_attr(doc, warn(missing_docs))]
#![no_std]

//! Zenith Grants Escrow — milestone-based escrow for community grants
//! and bounties.
//!
//! A funder locks the full grant upfront across N milestones, each with
//! its own amount and deadline. The grantee submits an evidence hash per
//! milestone; once `quorum` of the grant's reviewer committee approve it,
//! that milestone's amount is released. A milestone still unreleased
//! `GRACE_PERIOD` after its deadline can be reclaimed by the funder.
//! Partial milestone approval is not supported — a milestone pays out in
//! full or not at all. See docs/grants_escrow.md for operations.

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, BytesN, Env, Vec};

#[cfg(test)]
mod test;

mod error;
mod events;
mod migrations;
mod ttl;
mod types;

use error::Error;
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::DataKey;
pub use types::{Grant, Milestone, MilestoneStatus};

/// How long after a milestone's deadline reviewers still have to approve
/// a submission before the funder may reclaim it.
pub const GRACE_PERIOD: u64 = 7 * 86_400;
/// Maximum milestones per grant.
pub const MAX_MILESTONES: u32 = 20;
/// Maximum reviewers per grant.
pub const MAX_REVIEWERS: u32 = 20;
/// Maximum page size for listing calls.
pub const MAX_PAGE_SIZE: u32 = 50;

/// Milestone-based grant escrow with reviewer quorum release.
#[contract]
pub struct GrantsEscrow;

#[contractimpl]
impl GrantsEscrow {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs. Anyone may pay the rent to keep long-lived entries alive.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    /// Creates and fully funds a grant. `milestones` is a list of
    /// `(amount, deadline)`; the funder transfers their sum upfront.
    pub fn create_grant(
        env: Env,
        funder: Address,
        grantee: Address,
        token: Address,
        milestones: Vec<(i128, u64)>,
        reviewers: Vec<Address>,
        quorum: u32,
    ) -> u64 {
        ttl::extend_instance(&env);
        funder.require_auth();
        migrations::require_current(&env);

        if milestones.is_empty() || milestones.len() > MAX_MILESTONES {
            panic_with_error!(&env, Error::InvalidMilestones);
        }
        if reviewers.is_empty() || reviewers.len() > MAX_REVIEWERS {
            panic_with_error!(&env, Error::InvalidReviewers);
        }
        for i in 0..reviewers.len() {
            let reviewer = reviewers
                .get(i)
                .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
            if reviewer == grantee {
                panic_with_error!(&env, Error::ReviewerIsGrantee);
            }
            for j in (i + 1)..reviewers.len() {
                if reviewer
                    == reviewers
                        .get(j)
                        .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant))
                {
                    panic_with_error!(&env, Error::InvalidReviewers);
                }
            }
        }
        if quorum == 0 || quorum > reviewers.len() {
            panic_with_error!(&env, Error::InvalidQuorum);
        }

        let now = env.ledger().timestamp();
        let mut total: i128 = 0;
        let mut stored = Vec::new(&env);
        for (amount, deadline) in milestones.iter() {
            if amount <= 0 || deadline <= now {
                panic_with_error!(&env, Error::InvalidMilestones);
            }
            total = total
                .checked_add(amount)
                .unwrap_or_else(|| panic_with_error!(&env, Error::InvalidMilestones));
            stored.push_back(Milestone {
                amount,
                deadline,
                status: MilestoneStatus::Pending,
                evidence_hash: BytesN::from_array(&env, &[0; 32]),
                approvals: 0,
            });
        }

        token::Client::new(&env, &token).transfer(&funder, &env.current_contract_address(), &total);

        let grant_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::GrantCounter)
            .unwrap_or(0u64)
            + 1;
        env.storage()
            .instance()
            .set(&DataKey::GrantCounter, &grant_id);

        let grant = Grant {
            grant_id,
            funder: funder.clone(),
            grantee: grantee.clone(),
            token,
            reviewers,
            quorum,
            milestones: stored,
            created_at: now,
        };
        ttl::set_persistent(&env, &DataKey::Grant(grant_id), &grant);

        let index_key = DataKey::GranteeGrants(grantee.clone());
        let mut ids: Vec<u64> =
            ttl::get_persistent(&env, &index_key).unwrap_or_else(|| Vec::new(&env));
        ids.push_back(grant_id);
        ttl::set_persistent(&env, &index_key, &ids);

        events::grant_created(&env, grant_id, funder, grantee, total);
        grant_id
    }

    /// Grantee submits evidence for a pending milestone, before its deadline.
    pub fn submit_milestone(env: Env, grant_id: u64, idx: u32, evidence_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        let mut grant = Self::load(&env, grant_id);
        grant.grantee.require_auth();
        migrations::require_current(&env);

        let mut milestone = Self::milestone(&env, &grant, idx);
        if milestone.status != MilestoneStatus::Pending {
            panic_with_error!(&env, Error::InvalidMilestoneState);
        }
        if env.ledger().timestamp() > milestone.deadline {
            panic_with_error!(&env, Error::DeadlinePassed);
        }
        milestone.status = MilestoneStatus::Submitted;
        milestone.evidence_hash = evidence_hash.clone();
        grant.milestones.set(idx, milestone);
        ttl::set_persistent(&env, &DataKey::Grant(grant_id), &grant);

        events::milestone_submitted(&env, grant_id, idx, evidence_hash);
    }

    /// One reviewer's approval of a submitted milestone. The approval that
    /// reaches quorum releases the milestone's amount to the grantee.
    pub fn approve_milestone(env: Env, grant_id: u64, idx: u32, reviewer: Address) {
        ttl::extend_instance(&env);
        reviewer.require_auth();
        migrations::require_current(&env);
        let mut grant = Self::load(&env, grant_id);
        if !grant.reviewers.contains(&reviewer) {
            panic_with_error!(&env, Error::NotAReviewer);
        }

        let mut milestone = Self::milestone(&env, &grant, idx);
        if milestone.status != MilestoneStatus::Submitted {
            panic_with_error!(&env, Error::InvalidMilestoneState);
        }
        let approval_key = DataKey::Approval(grant_id, idx, reviewer.clone());
        if env.storage().persistent().has(&approval_key) {
            panic_with_error!(&env, Error::AlreadyApproved);
        }
        ttl::set_persistent(&env, &approval_key, &true);

        milestone.approvals += 1;
        events::milestone_approved(&env, grant_id, idx, reviewer, milestone.approvals);

        if milestone.approvals >= grant.quorum {
            milestone.status = MilestoneStatus::Released;
            token::Client::new(&env, &grant.token).transfer(
                &env.current_contract_address(),
                &grant.grantee,
                &milestone.amount,
            );
            events::milestone_released(&env, grant_id, idx, milestone.amount);
        }
        grant.milestones.set(idx, milestone);
        ttl::set_persistent(&env, &DataKey::Grant(grant_id), &grant);
    }

    /// Funder reclaims every unreleased milestone whose deadline plus
    /// `GRACE_PERIOD` has passed. Later milestones stay claimable by the
    /// grantee. Returns the amount reclaimed.
    pub fn reclaim(env: Env, grant_id: u64) -> i128 {
        ttl::extend_instance(&env);
        let mut grant = Self::load(&env, grant_id);
        grant.funder.require_auth();
        migrations::require_current(&env);

        let now = env.ledger().timestamp();
        let mut total: i128 = 0;
        for idx in 0..grant.milestones.len() {
            let mut milestone = grant
                .milestones
                .get(idx)
                .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
            let open = matches!(
                milestone.status,
                MilestoneStatus::Pending | MilestoneStatus::Submitted
            );
            if open && now > milestone.deadline.saturating_add(GRACE_PERIOD) {
                total += milestone.amount;
                milestone.status = MilestoneStatus::Reclaimed;
                grant.milestones.set(idx, milestone);
            }
        }
        if total == 0 {
            panic_with_error!(&env, Error::NothingToReclaim);
        }

        ttl::set_persistent(&env, &DataKey::Grant(grant_id), &grant);
        token::Client::new(&env, &grant.token).transfer(
            &env.current_contract_address(),
            &grant.funder,
            &total,
        );
        events::funds_reclaimed(&env, grant_id, total);
        total
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Returns the version
    /// reached (less than `to` if a step paginated). Permissionless:
    /// grants_escrow has no admin, and a new step can only arrive with a new
    /// wasm (see docs/migrations.md, "Governance decisions").
    pub fn migrate(env: Env, from: u32, to: u32) -> u32 {
        ttl::extend_instance(&env);
        zenith_common::migrations::migrate(
            &env,
            from,
            to,
            CURRENT_SCHEMA_VERSION,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by `migrate` and the steps are code in
    /// this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> u32 {
        ttl::extend_instance(&env);
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    /// Stored schema version. Read-only.
    pub fn schema_version(env: Env) -> u32 {
        ttl::extend_instance(&env);
        zenith_common::migrations::schema_version(&env)
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        ttl::extend_instance(&env);
        zenith_common::migrations::migration_state(&env)
    }

    /// Grant by id, if it exists. Read-only.
    pub fn get_grant(env: Env, grant_id: u64) -> Option<Grant> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::Grant(grant_id))
    }

    /// Number of grants created so far. Read-only.
    pub fn get_grant_count(env: Env) -> u64 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::GrantCounter)
            .unwrap_or(0)
    }

    /// Whether `reviewer` has approved milestone `idx` of grant `grant_id`. Read-only.
    pub fn has_approved(env: Env, grant_id: u64, idx: u32, reviewer: Address) -> bool {
        ttl::extend_instance(&env);
        env.storage()
            .persistent()
            .has(&DataKey::Approval(grant_id, idx, reviewer))
    }

    /// Number of grants for `grantee`. Read-only.
    pub fn get_grantee_grant_count(env: Env, grantee: Address) -> u32 {
        ttl::extend_instance(&env);
        let ids: Option<Vec<u64>> = ttl::get_persistent(&env, &DataKey::GranteeGrants(grantee));
        ids.map(|ids| ids.len()).unwrap_or(0)
    }

    /// Grant ids for `grantee` in creation order, `limit` (capped at
    /// `MAX_PAGE_SIZE`) starting at index `start`.
    pub fn get_grants_by_grantee(env: Env, grantee: Address, start: u32, limit: u32) -> Vec<u64> {
        ttl::extend_instance(&env);
        let ids: Vec<u64> = ttl::get_persistent(&env, &DataKey::GranteeGrants(grantee))
            .unwrap_or_else(|| Vec::new(&env));
        let end = start
            .saturating_add(limit.min(MAX_PAGE_SIZE))
            .min(ids.len());
        if start >= end {
            return Vec::new(&env);
        }
        ids.slice(start..end)
    }
}

impl GrantsEscrow {
    fn load(env: &Env, grant_id: u64) -> Grant {
        ttl::get_persistent(env, &DataKey::Grant(grant_id))
            .unwrap_or_else(|| panic_with_error!(env, Error::GrantNotFound))
    }

    fn milestone(env: &Env, grant: &Grant, idx: u32) -> Milestone {
        grant
            .milestones
            .get(idx)
            .unwrap_or_else(|| panic_with_error!(env, Error::MilestoneNotFound))
    }
}
