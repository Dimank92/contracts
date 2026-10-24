#![no_std]

//! Zenith Governor — on-chain proposals and voting for Zenith's admin
//! powers, modelled on OpenZeppelin Governor with the Timelock-control
//! extension.
//!
//! Holders of the checkpointed ZEN token `propose` batches of contract
//! calls, then vote For / Against / Abstain with their power as of the
//! proposal's snapshot ledger. Successful proposals are `queue`d into the
//! timelock, which later performs them when anyone calls its `execute`
//! with the proposal id. The timelock (not this contract) is what every
//! protocol contract's admin is set to, and the governor never sits in the
//! execution call stack — so a proposal can target the governor's own
//! parameter setters without tripping Soroban's no-re-entry rule.
//!
//! Attack resistance:
//! - Flash votes: power is read at the snapshot ledger, which is in the
//!   past by the time voting opens, so tokens acquired after proposal
//!   creation carry no weight.
//! - Spam: proposing needs `proposal_threshold` votes as of the previous
//!   ledger; if the proposer later drops below it, anyone may cancel.
//! - Quorum manipulation: quorum is a percentage of the total supply at
//!   the snapshot, not at vote time.
//!
//! Timepoints (`voting_delay`, `voting_period`, `snapshot`, `deadline`)
//! are ledger sequence numbers. See docs/governance.md.

use soroban_sdk::{
    contract, contractimpl, xdr::ToXdr, Address, BytesN, Env, IntoVal, Symbol, Val, Vec,
};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_events;
#[cfg(test)]
mod test_timelock;

mod error;
mod events;
pub mod interfaces;
mod migrations;
mod types;

pub use error::Error;
pub use events::{
    InitializedEvent, ParamSetEvent, ProposalCanceledEvent, ProposalCreatedEvent,
    ProposalQueuedEvent, VoteCastEvent,
};
use interfaces::{TimelockClient, VotesClient};
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::DataKey;
pub use types::{Proposal, ProposalState};

zenith_common::contract_meta!();

pub const SUPPORT_AGAINST: u32 = 0;
pub const SUPPORT_FOR: u32 = 1;
pub const SUPPORT_ABSTAIN: u32 = 2;

/// ~2 weeks of 5-second ledgers.
pub const MAX_VOTING_DELAY: u32 = 241_920;
/// ~1 hour of 5-second ledgers.
pub const MIN_VOTING_PERIOD: u32 = 720;
pub const MAX_VOTING_PERIOD: u32 = 241_920;
/// Quorum must be between 1% and 100% of past total supply.
pub const MIN_QUORUM_BPS: u32 = 100;
pub const MAX_QUORUM_BPS: u32 = 10_000;
pub const MAX_ACTIONS: u32 = 10;

#[contract]
pub struct Governor;

fn get<V: soroban_sdk::TryFromVal<Env, Val>>(env: &Env, key: &DataKey) -> Result<V, Error> {
    env.storage()
        .instance()
        .get(key)
        .ok_or(Error::NotInitialized)
}

fn load(env: &Env, id: &BytesN<32>) -> Result<Proposal, Error> {
    env.storage()
        .persistent()
        .get(&DataKey::Proposal(id.clone()))
        .ok_or(Error::ProposalNotFound)
}

fn save(env: &Env, id: &BytesN<32>, proposal: &Proposal) {
    env.storage()
        .persistent()
        .set(&DataKey::Proposal(id.clone()), proposal);
}

fn votes(env: &Env) -> Result<VotesClient<'_>, Error> {
    Ok(VotesClient::new(env, &get(env, &DataKey::Token)?))
}

fn timelock(env: &Env) -> Result<TimelockClient<'_>, Error> {
    Ok(TimelockClient::new(env, &get(env, &DataKey::Timelock)?))
}

fn check_bounds(ok: bool) -> Result<(), Error> {
    if !ok {
        return Err(Error::OutOfBounds);
    }
    Ok(())
}

/// Parameter changes are only accepted from the timelock, i.e. through a
/// successful proposal.
fn only_governance(env: &Env) -> Result<(), Error> {
    get::<Address>(env, &DataKey::Timelock)?.require_auth();
    Ok(())
}

/// Voting power as of the ledger before the current one.
fn power_now(env: &Env, account: &Address) -> Result<i128, Error> {
    let ledger = env.ledger().sequence().saturating_sub(1);
    Ok(votes(env)?.get_past_votes(account, &ledger))
}

fn add(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_add(b).ok_or(Error::MathOverflow)
}

fn quorum_at(env: &Env, ledger: u32) -> Result<i128, Error> {
    let supply = votes(env)?.get_past_total_supply(&ledger);
    let bps = get::<u32>(env, &DataKey::QuorumBps)? as i128;
    Ok(supply.checked_mul(bps).ok_or(Error::MathOverflow)? / 10_000)
}

fn state_of(env: &Env, id: &BytesN<32>, p: &Proposal) -> Result<ProposalState, Error> {
    if p.canceled {
        return Ok(ProposalState::Canceled);
    }
    if p.eta != 0 && timelock(env)?.is_done(id) {
        return Ok(ProposalState::Executed);
    }
    let seq = env.ledger().sequence();
    if seq <= p.snapshot {
        return Ok(ProposalState::Pending);
    }
    if seq <= p.deadline {
        return Ok(ProposalState::Active);
    }
    let quorum = quorum_at(env, p.snapshot)?;
    // A tie is a defeat.
    if add(p.for_votes, p.abstain_votes)? < quorum || p.for_votes <= p.against_votes {
        return Ok(ProposalState::Defeated);
    }
    if p.eta == 0 {
        return Ok(ProposalState::Succeeded);
    }
    let grace = timelock(env)?.get_grace_period();
    if env.ledger().timestamp() > p.eta.saturating_add(grace) {
        return Ok(ProposalState::Expired);
    }
    Ok(ProposalState::Queued)
}

fn cast(env: &Env, voter: Address, id: BytesN<32>, support: u32) -> Result<i128, Error> {
    migrations::require_current(env)?;
    let mut p = load(env, &id)?;
    if state_of(env, &id, &p)? != ProposalState::Active {
        return Err(Error::InvalidState);
    }
    let voted_key = DataKey::Voted(id.clone(), voter.clone());
    if env.storage().persistent().has(&voted_key) {
        return Err(Error::AlreadyVoted);
    }
    let weight = votes(env)?.get_past_votes(&voter, &p.snapshot);
    match support {
        SUPPORT_AGAINST => p.against_votes = add(p.against_votes, weight)?,
        SUPPORT_FOR => p.for_votes = add(p.for_votes, weight)?,
        SUPPORT_ABSTAIN => p.abstain_votes = add(p.abstain_votes, weight)?,
        _ => return Err(Error::InvalidSupport),
    }
    env.storage().persistent().set(&voted_key, &support);
    save(env, &id, &p);
    events::vote_cast(
        env,
        id,
        voter,
        VoteCastEvent {
            support,
            weight,
            for_votes: p.for_votes,
            against_votes: p.against_votes,
            abstain_votes: p.abstain_votes,
        },
    );
    Ok(weight)
}

#[contractimpl]
impl Governor {
    pub fn initialize(
        env: Env,
        token: Address,
        timelock: Address,
        voting_delay: u32,
        voting_period: u32,
        quorum_bps: u32,
        proposal_threshold: i128,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Token) {
            return Err(Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Timelock, &timelock);
        Self::apply_voting_delay(&env, voting_delay)?;
        Self::apply_voting_period(&env, voting_period)?;
        Self::apply_quorum_bps(&env, quorum_bps)?;
        Self::apply_proposal_threshold(&env, proposal_threshold)?;
        zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION);
        events::initialized(
            &env,
            InitializedEvent {
                token,
                timelock,
                voting_delay,
                voting_period,
                quorum_bps,
                proposal_threshold,
                schema_version: CURRENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    /// Deterministic proposal id: sha256 over the XDR of all four inputs.
    pub fn hash_proposal(
        env: Env,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        description_hash: BytesN<32>,
    ) -> Result<BytesN<32>, Error> {
        Ok(env
            .crypto()
            .sha256(&(targets, fns, args, description_hash).to_xdr(&env))
            .into())
    }

    pub fn propose(
        env: Env,
        proposer: Address,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        description_hash: BytesN<32>,
    ) -> Result<BytesN<32>, Error> {
        proposer.require_auth();
        migrations::require_current(&env)?;
        let n = targets.len();
        if n == 0 || n > MAX_ACTIONS || n != fns.len() || n != args.len() {
            return Err(Error::InvalidProposal);
        }
        if power_now(&env, &proposer)? < get::<i128>(&env, &DataKey::ProposalThreshold)? {
            return Err(Error::BelowProposalThreshold);
        }

        let id = Self::hash_proposal(
            env.clone(),
            targets.clone(),
            fns.clone(),
            args.clone(),
            description_hash.clone(),
        )?;
        if env
            .storage()
            .persistent()
            .has(&DataKey::Proposal(id.clone()))
        {
            return Err(Error::ProposalExists);
        }

        let seq = env.ledger().sequence();
        let snapshot = seq
            .checked_add(get(&env, &DataKey::VotingDelay)?)
            .ok_or(Error::MathOverflow)?;
        let deadline = snapshot
            .checked_add(get(&env, &DataKey::VotingPeriod)?)
            .ok_or(Error::MathOverflow)?;
        save(
            &env,
            &id,
            &Proposal {
                proposer: proposer.clone(),
                targets: targets.clone(),
                fns: fns.clone(),
                args: args.clone(),
                snapshot,
                deadline,
                for_votes: 0,
                against_votes: 0,
                abstain_votes: 0,
                canceled: false,
                eta: 0,
            },
        );
        events::proposal_created(
            &env,
            id.clone(),
            ProposalCreatedEvent {
                proposer,
                targets,
                fns,
                args,
                description_hash,
                snapshot,
                deadline,
            },
        );
        Ok(id)
    }

    /// `support`: 0 = Against, 1 = For, 2 = Abstain. Returns the weight.
    pub fn cast_vote(
        env: Env,
        voter: Address,
        id: BytesN<32>,
        support: u32,
    ) -> Result<i128, Error> {
        voter.require_auth();
        cast(&env, voter, id, support)
    }

    /// Gasless voting. On Soroban a "signature" is the voter's signed
    /// authorization entry: the voter signs off-chain an authorization for
    /// exactly `(id, support)` on this contract, and any relayer submits
    /// the transaction. The host verifies the signature and its nonce, so
    /// it cannot be replayed or reused for a different proposal or choice.
    pub fn cast_vote_by_sig(
        env: Env,
        voter: Address,
        id: BytesN<32>,
        support: u32,
    ) -> Result<i128, Error> {
        voter.require_auth_for_args((id.clone(), support).into_val(&env));
        cast(&env, voter, id, support)
    }

    /// Hands a succeeded proposal to the timelock with its minimum delay.
    pub fn queue(env: Env, id: BytesN<32>) -> Result<u64, Error> {
        migrations::require_current(&env)?;
        let mut p = load(&env, &id)?;
        if state_of(&env, &id, &p)? != ProposalState::Succeeded {
            return Err(Error::InvalidState);
        }
        let tl = timelock(&env)?;
        let eta = tl.schedule(&id, &p.targets, &p.fns, &p.args, &tl.get_min_delay());
        p.eta = eta;
        save(&env, &id, &p);
        events::proposal_queued(
            &env,
            id,
            ProposalQueuedEvent {
                eta,
                quorum: quorum_at(&env, p.snapshot)?,
            },
        );
        Ok(eta)
    }

    /// The proposer may cancel while Pending. Anyone may cancel a
    /// not-yet-executed proposal whose proposer's current voting power
    /// has fallen below the proposal threshold.
    pub fn cancel(env: Env, caller: Address, id: BytesN<32>) -> Result<(), Error> {
        caller.require_auth();
        migrations::require_current(&env)?;
        let mut p = load(&env, &id)?;
        let state = state_of(&env, &id, &p)?;
        if !matches!(
            state,
            ProposalState::Pending
                | ProposalState::Active
                | ProposalState::Succeeded
                | ProposalState::Queued
        ) {
            return Err(Error::InvalidState);
        }
        let by_proposer = caller == p.proposer && state == ProposalState::Pending;
        let threshold: i128 = get(&env, &DataKey::ProposalThreshold)?;
        if !by_proposer && power_now(&env, &p.proposer)? >= threshold {
            return Err(Error::Unauthorized);
        }
        p.canceled = true;
        save(&env, &id, &p);
        let was_queued = state == ProposalState::Queued;
        if was_queued {
            timelock(&env)?.cancel(&env.current_contract_address(), &id);
        }
        events::proposal_canceled(&env, id, ProposalCanceledEvent { caller, was_queued });
        Ok(())
    }

    // ── Governance-settable parameters (timelock only) ───────────────────────

    pub fn set_voting_delay(env: Env, value: u32) -> Result<(), Error> {
        only_governance(&env)?;
        migrations::require_current(&env)?;
        Self::apply_voting_delay(&env, value)
    }

    pub fn set_voting_period(env: Env, value: u32) -> Result<(), Error> {
        only_governance(&env)?;
        migrations::require_current(&env)?;
        Self::apply_voting_period(&env, value)
    }

    pub fn set_quorum_bps(env: Env, value: u32) -> Result<(), Error> {
        only_governance(&env)?;
        migrations::require_current(&env)?;
        Self::apply_quorum_bps(&env, value)
    }

    pub fn set_proposal_threshold(env: Env, value: i128) -> Result<(), Error> {
        only_governance(&env)?;
        migrations::require_current(&env)?;
        Self::apply_proposal_threshold(&env, value)
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Returns the version
    /// reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, from: u32, to: u32) -> Result<u32, Error> {
        only_governance(&env)?;
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
    /// `bump`: the target was fixed by the authorized `migrate` call and the
    /// steps are code in this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> Result<u32, Error> {
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    // ── Views ────────────────────────────────────────────────────────────────

    pub fn schema_version(env: Env) -> Result<u32, Error> {
        Ok(zenith_common::migrations::schema_version(&env))
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Result<Option<(u32, u32)>, Error> {
        Ok(zenith_common::migrations::migration_state(&env))
    }

    pub fn state(env: Env, id: BytesN<32>) -> Result<ProposalState, Error> {
        state_of(&env, &id, &load(&env, &id)?)
    }

    pub fn get_proposal(env: Env, id: BytesN<32>) -> Result<Option<Proposal>, Error> {
        Ok(env.storage().persistent().get(&DataKey::Proposal(id)))
    }

    pub fn has_voted(env: Env, id: BytesN<32>, voter: Address) -> Result<bool, Error> {
        Ok(env.storage().persistent().has(&DataKey::Voted(id, voter)))
    }

    /// Votes (For + Abstain) required for a proposal snapshotted at `ledger`.
    pub fn quorum(env: Env, ledger: u32) -> Result<i128, Error> {
        quorum_at(&env, ledger)
    }

    pub fn voting_delay(env: Env) -> Result<u32, Error> {
        get(&env, &DataKey::VotingDelay)
    }

    pub fn voting_period(env: Env) -> Result<u32, Error> {
        get(&env, &DataKey::VotingPeriod)
    }

    pub fn quorum_bps(env: Env) -> Result<u32, Error> {
        get(&env, &DataKey::QuorumBps)
    }

    pub fn proposal_threshold(env: Env) -> Result<i128, Error> {
        get(&env, &DataKey::ProposalThreshold)
    }

    pub fn get_token(env: Env) -> Result<Address, Error> {
        get(&env, &DataKey::Token)
    }

    pub fn get_timelock(env: Env) -> Result<Address, Error> {
        get(&env, &DataKey::Timelock)
    }
}

impl Governor {
    fn apply_voting_delay(env: &Env, value: u32) -> Result<(), Error> {
        check_bounds(value <= MAX_VOTING_DELAY)?;
        env.storage().instance().set(&DataKey::VotingDelay, &value);
        events::param_set(env, "voting_delay", value as i128);
        Ok(())
    }

    fn apply_voting_period(env: &Env, value: u32) -> Result<(), Error> {
        check_bounds((MIN_VOTING_PERIOD..=MAX_VOTING_PERIOD).contains(&value))?;
        env.storage().instance().set(&DataKey::VotingPeriod, &value);
        events::param_set(env, "voting_period", value as i128);
        Ok(())
    }

    fn apply_quorum_bps(env: &Env, value: u32) -> Result<(), Error> {
        check_bounds((MIN_QUORUM_BPS..=MAX_QUORUM_BPS).contains(&value))?;
        env.storage().instance().set(&DataKey::QuorumBps, &value);
        events::param_set(env, "quorum_bps", value as i128);
        Ok(())
    }

    fn apply_proposal_threshold(env: &Env, value: i128) -> Result<(), Error> {
        check_bounds(value >= 0)?;
        env.storage()
            .instance()
            .set(&DataKey::ProposalThreshold, &value);
        events::param_set(env, "proposal_threshold", value);
        Ok(())
    }
}
