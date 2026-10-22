// Public API docs are enforced by `cargo xtask doc` (#268): the gate builds
// rustdoc, where this lint is on, and only there — soroban-sdk macro output is
// undocumented, which would otherwise fail `clippy -D warnings`.
#![cfg_attr(doc, warn(missing_docs))]
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
    contract, contractimpl, panic_with_error, xdr::ToXdr, Address, BytesN, Env, IntoVal, Symbol,
    Val, Vec,
};

#[cfg(test)]
mod test;

mod error;
mod events;
pub mod interfaces;
mod migrations;
mod types;

use error::Error;
use interfaces::{TimelockClient, VotesClient};
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::DataKey;
pub use types::{Proposal, ProposalState};

/// Vote support value: against.
pub const SUPPORT_AGAINST: u32 = 0;
/// Vote support value: for.
pub const SUPPORT_FOR: u32 = 1;
/// Vote support value: abstain.
pub const SUPPORT_ABSTAIN: u32 = 2;

/// ~2 weeks of 5-second ledgers.
pub const MAX_VOTING_DELAY: u32 = 241_920;
/// ~1 hour of 5-second ledgers.
pub const MIN_VOTING_PERIOD: u32 = 720;
/// Upper bound on the voting period, in ledgers.
pub const MAX_VOTING_PERIOD: u32 = 241_920;
/// Quorum must be between 1% and 100% of past total supply.
pub const MIN_QUORUM_BPS: u32 = 100;
/// Upper bound on the quorum, in basis points.
pub const MAX_QUORUM_BPS: u32 = 10_000;
/// Maximum actions per proposal.
pub const MAX_ACTIONS: u32 = 10;

/// Token-weighted governor that queues approved proposals on the timelock.
#[contract]
pub struct Governor;

fn get<V: soroban_sdk::TryFromVal<Env, Val>>(env: &Env, key: &DataKey) -> V {
    env.storage()
        .instance()
        .get(key)
        .unwrap_or_else(|| panic_with_error!(env, Error::InternalInvariant))
}

fn load(env: &Env, id: &BytesN<32>) -> Proposal {
    env.storage()
        .persistent()
        .get(&DataKey::Proposal(id.clone()))
        .unwrap_or_else(|| panic_with_error!(env, Error::ProposalNotFound))
}

fn save(env: &Env, id: &BytesN<32>, proposal: &Proposal) {
    env.storage()
        .persistent()
        .set(&DataKey::Proposal(id.clone()), proposal);
}

fn votes(env: &Env) -> VotesClient<'_> {
    VotesClient::new(env, &get(env, &DataKey::Token))
}

fn timelock(env: &Env) -> TimelockClient<'_> {
    TimelockClient::new(env, &get(env, &DataKey::Timelock))
}

fn check_bounds(env: &Env, ok: bool) {
    if !ok {
        panic_with_error!(env, Error::OutOfBounds);
    }
}

/// Parameter changes are only accepted from the timelock, i.e. through a
/// successful proposal.
fn only_governance(env: &Env) {
    get::<Address>(env, &DataKey::Timelock).require_auth();
}

fn state_of(env: &Env, id: &BytesN<32>, p: &Proposal) -> ProposalState {
    if p.canceled {
        return ProposalState::Canceled;
    }
    if p.eta != 0 && timelock(env).is_done(id) {
        return ProposalState::Executed;
    }
    let seq = env.ledger().sequence();
    if seq <= p.snapshot {
        return ProposalState::Pending;
    }
    if seq <= p.deadline {
        return ProposalState::Active;
    }
    let quorum = Governor::quorum(env.clone(), p.snapshot);
    // A tie is a defeat.
    if p.for_votes + p.abstain_votes < quorum || p.for_votes <= p.against_votes {
        return ProposalState::Defeated;
    }
    if p.eta == 0 {
        return ProposalState::Succeeded;
    }
    if env.ledger().timestamp() > p.eta + timelock(env).get_grace_period() {
        return ProposalState::Expired;
    }
    ProposalState::Queued
}

fn cast(env: &Env, voter: Address, id: BytesN<32>, support: u32) -> i128 {
    migrations::require_current(env);
    let mut p = load(env, &id);
    if state_of(env, &id, &p) != ProposalState::Active {
        panic_with_error!(env, Error::InvalidState);
    }
    let voted_key = DataKey::Voted(id.clone(), voter.clone());
    if env.storage().persistent().has(&voted_key) {
        panic_with_error!(env, Error::AlreadyVoted);
    }
    let weight = votes(env).get_past_votes(&voter, &p.snapshot);
    match support {
        SUPPORT_AGAINST => p.against_votes += weight,
        SUPPORT_FOR => p.for_votes += weight,
        SUPPORT_ABSTAIN => p.abstain_votes += weight,
        _ => panic_with_error!(env, Error::InvalidSupport),
    }
    env.storage().persistent().set(&voted_key, &support);
    save(env, &id, &p);
    events::vote_cast(env, id, voter, support, weight);
    weight
}

#[contractimpl]
impl Governor {
    /// Binds the voting `token` and `timelock` and sets the initial parameters. Fails with `AlreadyInitialized` on a second call.
    pub fn initialize(
        env: Env,
        token: Address,
        timelock: Address,
        voting_delay: u32,
        voting_period: u32,
        quorum_bps: u32,
        proposal_threshold: i128,
    ) {
        if env.storage().instance().has(&DataKey::Token) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Timelock, &timelock);
        Self::apply_voting_delay(&env, voting_delay);
        Self::apply_voting_period(&env, voting_period);
        Self::apply_quorum_bps(&env, quorum_bps);
        Self::apply_proposal_threshold(&env, proposal_threshold);
        zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION);
    }

    /// Deterministic proposal id: sha256 over the XDR of all four inputs.
    pub fn hash_proposal(
        env: Env,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        description_hash: BytesN<32>,
    ) -> BytesN<32> {
        env.crypto()
            .sha256(&(targets, fns, args, description_hash).to_xdr(&env))
            .into()
    }

    /// Creates a proposal of up to `MAX_ACTIONS` calls and returns its id. Requires `proposer` auth; fails with `InvalidProposal` on malformed actions.
    pub fn propose(
        env: Env,
        proposer: Address,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        description_hash: BytesN<32>,
    ) -> BytesN<32> {
        proposer.require_auth();
        migrations::require_current(&env);
        let n = targets.len();
        if n == 0 || n > MAX_ACTIONS || n != fns.len() || n != args.len() {
            panic_with_error!(&env, Error::InvalidProposal);
        }
        let seq = env.ledger().sequence();
        let power = votes(&env).get_past_votes(&proposer, &(seq - 1));
        if power < Self::proposal_threshold(env.clone()) {
            panic_with_error!(&env, Error::BelowProposalThreshold);
        }

        let id = Self::hash_proposal(
            env.clone(),
            targets.clone(),
            fns.clone(),
            args.clone(),
            description_hash,
        );
        if env
            .storage()
            .persistent()
            .has(&DataKey::Proposal(id.clone()))
        {
            panic_with_error!(&env, Error::ProposalExists);
        }

        let snapshot = seq + Self::voting_delay(env.clone());
        let deadline = snapshot + Self::voting_period(env.clone());
        save(
            &env,
            &id,
            &Proposal {
                proposer: proposer.clone(),
                targets,
                fns,
                args,
                snapshot,
                deadline,
                for_votes: 0,
                against_votes: 0,
                abstain_votes: 0,
                canceled: false,
                eta: 0,
            },
        );
        events::proposal_created(&env, id.clone(), proposer, snapshot, deadline);
        id
    }

    /// `support`: 0 = Against, 1 = For, 2 = Abstain. Returns the weight.
    pub fn cast_vote(env: Env, voter: Address, id: BytesN<32>, support: u32) -> i128 {
        voter.require_auth();
        cast(&env, voter, id, support)
    }

    /// Gasless voting. On Soroban a "signature" is the voter's signed
    /// authorization entry: the voter signs off-chain an authorization for
    /// exactly `(id, support)` on this contract, and any relayer submits
    /// the transaction. The host verifies the signature and its nonce, so
    /// it cannot be replayed or reused for a different proposal or choice.
    pub fn cast_vote_by_sig(env: Env, voter: Address, id: BytesN<32>, support: u32) -> i128 {
        voter.require_auth_for_args((id.clone(), support).into_val(&env));
        cast(&env, voter, id, support)
    }

    /// Hands a succeeded proposal to the timelock with its minimum delay.
    pub fn queue(env: Env, id: BytesN<32>) -> u64 {
        migrations::require_current(&env);
        let mut p = load(&env, &id);
        if state_of(&env, &id, &p) != ProposalState::Succeeded {
            panic_with_error!(&env, Error::InvalidState);
        }
        let tl = timelock(&env);
        let eta = tl.schedule(&id, &p.targets, &p.fns, &p.args, &tl.get_min_delay());
        p.eta = eta;
        save(&env, &id, &p);
        events::proposal_queued(&env, id, eta);
        eta
    }

    /// The proposer may cancel while Pending. Anyone may cancel a
    /// not-yet-executed proposal whose proposer's current voting power
    /// has fallen below the proposal threshold.
    pub fn cancel(env: Env, caller: Address, id: BytesN<32>) {
        caller.require_auth();
        migrations::require_current(&env);
        let mut p = load(&env, &id);
        let state = state_of(&env, &id, &p);
        if !matches!(
            state,
            ProposalState::Pending
                | ProposalState::Active
                | ProposalState::Succeeded
                | ProposalState::Queued
        ) {
            panic_with_error!(&env, Error::InvalidState);
        }
        let by_proposer = caller == p.proposer && state == ProposalState::Pending;
        let power = votes(&env).get_past_votes(&p.proposer, &(env.ledger().sequence() - 1));
        if !by_proposer && power >= Self::proposal_threshold(env.clone()) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        p.canceled = true;
        save(&env, &id, &p);
        if state == ProposalState::Queued {
            timelock(&env).cancel(&env.current_contract_address(), &id);
        }
        events::proposal_canceled(&env, id);
    }

    // ── Governance-settable parameters (timelock only) ───────────────────────

    /// Sets the voting delay in ledgers. Governance (timelock) only.
    pub fn set_voting_delay(env: Env, value: u32) {
        only_governance(&env);
        migrations::require_current(&env);
        Self::apply_voting_delay(&env, value);
    }

    /// Sets the voting period in ledgers. Governance (timelock) only.
    pub fn set_voting_period(env: Env, value: u32) {
        only_governance(&env);
        migrations::require_current(&env);
        Self::apply_voting_period(&env, value);
    }

    /// Sets the quorum in basis points. Governance (timelock) only.
    pub fn set_quorum_bps(env: Env, value: u32) {
        only_governance(&env);
        migrations::require_current(&env);
        Self::apply_quorum_bps(&env, value);
    }

    /// Sets the minimum voting power needed to propose. Governance (timelock) only.
    pub fn set_proposal_threshold(env: Env, value: i128) {
        only_governance(&env);
        migrations::require_current(&env);
        Self::apply_proposal_threshold(&env, value);
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Returns the version
    /// reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, from: u32, to: u32) -> u32 {
        only_governance(&env);
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
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> u32 {
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    // ── Views ────────────────────────────────────────────────────────────────

    /// Stored schema version. Read-only.
    pub fn schema_version(env: Env) -> u32 {
        zenith_common::migrations::schema_version(&env)
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        zenith_common::migrations::migration_state(&env)
    }

    /// Current state of proposal `id`. Read-only.
    pub fn state(env: Env, id: BytesN<32>) -> ProposalState {
        state_of(&env, &id, &load(&env, &id))
    }

    /// Proposal by id, if it exists. Read-only.
    pub fn get_proposal(env: Env, id: BytesN<32>) -> Option<Proposal> {
        env.storage().persistent().get(&DataKey::Proposal(id))
    }

    /// Whether `voter` has voted on proposal `id`. Read-only.
    pub fn has_voted(env: Env, id: BytesN<32>, voter: Address) -> bool {
        env.storage().persistent().has(&DataKey::Voted(id, voter))
    }

    /// Votes (For + Abstain) required for a proposal snapshotted at `ledger`.
    pub fn quorum(env: Env, ledger: u32) -> i128 {
        let supply = votes(&env).get_past_total_supply(&ledger);
        let bps = get::<u32>(&env, &DataKey::QuorumBps) as i128;
        supply
            .checked_mul(bps)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant))
            / 10_000
    }

    /// Voting delay in ledgers. Read-only.
    pub fn voting_delay(env: Env) -> u32 {
        get(&env, &DataKey::VotingDelay)
    }

    /// Voting period in ledgers. Read-only.
    pub fn voting_period(env: Env) -> u32 {
        get(&env, &DataKey::VotingPeriod)
    }

    /// Quorum in basis points. Read-only.
    pub fn quorum_bps(env: Env) -> u32 {
        get(&env, &DataKey::QuorumBps)
    }

    /// Voting power required to propose. Read-only.
    pub fn proposal_threshold(env: Env) -> i128 {
        get(&env, &DataKey::ProposalThreshold)
    }

    /// Voting token address. Read-only.
    pub fn get_token(env: Env) -> Address {
        get(&env, &DataKey::Token)
    }

    /// Timelock address. Read-only.
    pub fn get_timelock(env: Env) -> Address {
        get(&env, &DataKey::Timelock)
    }
}

impl Governor {
    fn apply_voting_delay(env: &Env, value: u32) {
        check_bounds(env, value <= MAX_VOTING_DELAY);
        env.storage().instance().set(&DataKey::VotingDelay, &value);
        events::param_set(env, "voting_delay", value as i128);
    }

    fn apply_voting_period(env: &Env, value: u32) {
        check_bounds(
            env,
            (MIN_VOTING_PERIOD..=MAX_VOTING_PERIOD).contains(&value),
        );
        env.storage().instance().set(&DataKey::VotingPeriod, &value);
        events::param_set(env, "voting_period", value as i128);
    }

    fn apply_quorum_bps(env: &Env, value: u32) {
        check_bounds(env, (MIN_QUORUM_BPS..=MAX_QUORUM_BPS).contains(&value));
        env.storage().instance().set(&DataKey::QuorumBps, &value);
        events::param_set(env, "quorum_bps", value as i128);
    }

    fn apply_proposal_threshold(env: &Env, value: i128) {
        check_bounds(env, value >= 0);
        env.storage()
            .instance()
            .set(&DataKey::ProposalThreshold, &value);
        events::param_set(env, "proposal_threshold", value);
    }
}
