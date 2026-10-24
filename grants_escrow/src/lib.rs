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

use soroban_sdk::{contract, contractimpl, token, Address, BytesN, Env, Vec};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_events;

mod error;
mod events;
mod migrations;
mod ttl;
mod types;

pub use error::Error;
pub use events::{
    FundsReclaimedEvent, GrantCreatedEvent, MilestoneApprovedEvent, MilestoneReleasedEvent,
    MilestoneSubmittedEvent,
};
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::DataKey;
pub use types::{Grant, Milestone, MilestoneStatus};

zenith_common::contract_meta!();

/// How long after a milestone's deadline reviewers still have to approve
/// a submission before the funder may reclaim it.
pub const GRACE_PERIOD: u64 = 7 * 86_400;
pub const MAX_MILESTONES: u32 = 20;
pub const MAX_REVIEWERS: u32 = 20;
pub const MAX_PAGE_SIZE: u32 = 50;

#[contract]
pub struct GrantsEscrow;

#[contractimpl]
impl GrantsEscrow {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs. Anyone may pay the rent to keep long-lived entries alive.
    pub fn bump(env: Env, keys: Vec<DataKey>) -> Result<(), Error> {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
        Ok(())
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
    ) -> Result<u64, Error> {
        ttl::extend_instance(&env);
        funder.require_auth();
        migrations::require_current(&env)?;

        if milestones.is_empty() || milestones.len() > MAX_MILESTONES {
            return Err(Error::InvalidMilestones);
        }
        if reviewers.is_empty() || reviewers.len() > MAX_REVIEWERS {
            return Err(Error::InvalidReviewers);
        }
        for (i, reviewer) in reviewers.iter().enumerate() {
            if reviewer == grantee {
                return Err(Error::ReviewerIsGrantee);
            }
            if reviewers.iter().skip(i + 1).any(|other| other == reviewer) {
                return Err(Error::InvalidReviewers);
            }
        }
        if quorum == 0 || quorum > reviewers.len() {
            return Err(Error::InvalidQuorum);
        }

        let now = env.ledger().timestamp();
        let mut total: i128 = 0;
        let mut stored = Vec::new(&env);
        let mut amounts = Vec::new(&env);
        let mut deadlines = Vec::new(&env);
        for (amount, deadline) in milestones.iter() {
            if amount <= 0 || deadline <= now {
                return Err(Error::InvalidMilestones);
            }
            total = total.checked_add(amount).ok_or(Error::InvalidMilestones)?;
            amounts.push_back(amount);
            deadlines.push_back(deadline);
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
            .checked_add(1)
            .ok_or(Error::MathOverflow)?;
        env.storage()
            .instance()
            .set(&DataKey::GrantCounter, &grant_id);

        let grant = Grant {
            grant_id,
            funder: funder.clone(),
            grantee: grantee.clone(),
            token: token.clone(),
            reviewers: reviewers.clone(),
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

        events::grant_created(
            &env,
            grant_id,
            GrantCreatedEvent {
                funder,
                grantee,
                token,
                total,
                reviewers,
                quorum,
                amounts,
                deadlines,
                created_at: now,
            },
        );
        Ok(grant_id)
    }

    /// Grantee submits evidence for a pending milestone, before its deadline.
    pub fn submit_milestone(
        env: Env,
        grant_id: u64,
        idx: u32,
        evidence_hash: BytesN<32>,
    ) -> Result<(), Error> {
        ttl::extend_instance(&env);
        let mut grant = Self::load(&env, grant_id)?;
        grant.grantee.require_auth();
        migrations::require_current(&env)?;

        let mut milestone = Self::milestone(&grant, idx)?;
        if milestone.status != MilestoneStatus::Pending {
            return Err(Error::InvalidMilestoneState);
        }
        let now = env.ledger().timestamp();
        if now > milestone.deadline {
            return Err(Error::DeadlinePassed);
        }
        milestone.status = MilestoneStatus::Submitted;
        milestone.evidence_hash = evidence_hash.clone();
        grant.milestones.set(idx, milestone);
        ttl::set_persistent(&env, &DataKey::Grant(grant_id), &grant);

        events::milestone_submitted(
            &env,
            grant_id,
            idx,
            MilestoneSubmittedEvent {
                evidence_hash,
                submitted_at: now,
            },
        );
        Ok(())
    }

    /// One reviewer's approval of a submitted milestone. The approval that
    /// reaches quorum releases the milestone's amount to the grantee.
    pub fn approve_milestone(
        env: Env,
        grant_id: u64,
        idx: u32,
        reviewer: Address,
    ) -> Result<(), Error> {
        ttl::extend_instance(&env);
        reviewer.require_auth();
        migrations::require_current(&env)?;
        let mut grant = Self::load(&env, grant_id)?;
        if !grant.reviewers.contains(&reviewer) {
            return Err(Error::NotAReviewer);
        }

        let mut milestone = Self::milestone(&grant, idx)?;
        if milestone.status != MilestoneStatus::Submitted {
            return Err(Error::InvalidMilestoneState);
        }
        let approval_key = DataKey::Approval(grant_id, idx, reviewer.clone());
        if env.storage().persistent().has(&approval_key) {
            return Err(Error::AlreadyApproved);
        }
        ttl::set_persistent(&env, &approval_key, &true);

        // Bounded by the reviewer count (at most MAX_REVIEWERS).
        milestone.approvals += 1;
        events::milestone_approved(
            &env,
            grant_id,
            idx,
            MilestoneApprovedEvent {
                reviewer,
                approvals: milestone.approvals,
                quorum: grant.quorum,
            },
        );

        let released = milestone.approvals >= grant.quorum;
        let amount = milestone.amount;
        if released {
            milestone.status = MilestoneStatus::Released;
        }
        grant.milestones.set(idx, milestone);
        ttl::set_persistent(&env, &DataKey::Grant(grant_id), &grant);
        if released {
            token::Client::new(&env, &grant.token).transfer(
                &env.current_contract_address(),
                &grant.grantee,
                &amount,
            );
            events::milestone_released(
                &env,
                grant_id,
                idx,
                MilestoneReleasedEvent {
                    grantee: grant.grantee,
                    amount,
                },
            );
        }
        Ok(())
    }

    /// Funder reclaims every unreleased milestone whose deadline plus
    /// `GRACE_PERIOD` has passed. Later milestones stay claimable by the
    /// grantee. Returns the amount reclaimed.
    pub fn reclaim(env: Env, grant_id: u64) -> Result<i128, Error> {
        ttl::extend_instance(&env);
        let mut grant = Self::load(&env, grant_id)?;
        grant.funder.require_auth();
        migrations::require_current(&env)?;

        let now = env.ledger().timestamp();
        let mut total: i128 = 0;
        let mut reclaimed = Vec::new(&env);
        let mut milestones = grant.milestones.clone();
        for (idx, mut milestone) in grant.milestones.iter().enumerate() {
            let open = matches!(
                milestone.status,
                MilestoneStatus::Pending | MilestoneStatus::Submitted
            );
            if open && now > milestone.deadline.saturating_add(GRACE_PERIOD) {
                // Bounded by the grant's checked total at creation.
                total += milestone.amount;
                milestone.status = MilestoneStatus::Reclaimed;
                milestones.set(idx as u32, milestone);
                reclaimed.push_back(idx as u32);
            }
        }
        if total == 0 {
            return Err(Error::NothingToReclaim);
        }
        grant.milestones = milestones;

        ttl::set_persistent(&env, &DataKey::Grant(grant_id), &grant);
        token::Client::new(&env, &grant.token).transfer(
            &env.current_contract_address(),
            &grant.funder,
            &total,
        );
        events::funds_reclaimed(
            &env,
            grant_id,
            FundsReclaimedEvent {
                funder: grant.funder,
                amount: total,
                milestones: reclaimed,
            },
        );
        Ok(total)
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Returns the version
    /// reached (less than `to` if a step paginated). Permissionless:
    /// grants_escrow has no admin, and a new step can only arrive with a new
    /// wasm (see docs/migrations.md, "Governance decisions").
    pub fn migrate(env: Env, from: u32, to: u32) -> Result<u32, Error> {
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
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    pub fn schema_version(env: Env) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        Ok(zenith_common::migrations::schema_version(&env))
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Result<Option<(u32, u32)>, Error> {
        ttl::extend_instance(&env);
        Ok(zenith_common::migrations::migration_state(&env))
    }

    pub fn get_grant(env: Env, grant_id: u64) -> Result<Option<Grant>, Error> {
        ttl::extend_instance(&env);
        Ok(ttl::get_persistent(&env, &DataKey::Grant(grant_id)))
    }

    pub fn get_grant_count(env: Env) -> Result<u64, Error> {
        ttl::extend_instance(&env);
        Ok(env
            .storage()
            .instance()
            .get(&DataKey::GrantCounter)
            .unwrap_or(0))
    }

    pub fn has_approved(
        env: Env,
        grant_id: u64,
        idx: u32,
        reviewer: Address,
    ) -> Result<bool, Error> {
        ttl::extend_instance(&env);
        Ok(env
            .storage()
            .persistent()
            .has(&DataKey::Approval(grant_id, idx, reviewer)))
    }

    pub fn get_grantee_grant_count(env: Env, grantee: Address) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        let ids: Option<Vec<u64>> = ttl::get_persistent(&env, &DataKey::GranteeGrants(grantee));
        Ok(ids.map(|ids| ids.len()).unwrap_or(0))
    }

    /// Grant ids for `grantee` in creation order, `limit` (capped at
    /// `MAX_PAGE_SIZE`) starting at index `start`.
    pub fn get_grants_by_grantee(
        env: Env,
        grantee: Address,
        start: u32,
        limit: u32,
    ) -> Result<Vec<u64>, Error> {
        ttl::extend_instance(&env);
        let ids: Vec<u64> = ttl::get_persistent(&env, &DataKey::GranteeGrants(grantee))
            .unwrap_or_else(|| Vec::new(&env));
        let end = start
            .saturating_add(limit.min(MAX_PAGE_SIZE))
            .min(ids.len());
        if start >= end {
            return Ok(Vec::new(&env));
        }
        Ok(ids.slice(start..end))
    }
}

impl GrantsEscrow {
    fn load(env: &Env, grant_id: u64) -> Result<Grant, Error> {
        ttl::get_persistent(env, &DataKey::Grant(grant_id)).ok_or(Error::GrantNotFound)
    }

    fn milestone(grant: &Grant, idx: u32) -> Result<Milestone, Error> {
        grant.milestones.get(idx).ok_or(Error::MilestoneNotFound)
    }
}
