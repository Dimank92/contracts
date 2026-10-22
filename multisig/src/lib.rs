// Public API docs are enforced by `cargo xtask doc` (#268): the gate builds
// rustdoc, where this lint is on, and only there — soroban-sdk macro output is
// undocumented, which would otherwise fail `clippy -D warnings`.
#![cfg_attr(doc, warn(missing_docs))]
#![no_std]

//! Zenith Multisig — weighted approval tracking, an on-chain action
//! registry, a proposal executor, and governed signer rotation.
//!
//! Two ways to use it:
//! - **Vote counter** (`approve` / `is_approved`): consumers' existing
//!   `_via_multisig` entrypoints check `is_approved(action_id)` for an
//!   opaque, caller-defined `BytesN<32>` id.
//! - **Executor** (`propose` / `approve` / `execute`): the multisig is the
//!   target contract's admin and, once approved, calls
//!   `target.function(args)` itself, satisfying `admin.require_auth()`.
//!
//! Every signer carries a weight, and an action is approved once the
//! summed weight of its fresh approvals reaches `threshold`. The legacy
//! `initialize` gives every signer weight 1, which is plain M-of-N.
//!
//! Every action gets an `ActionMeta` registry entry (proposer, creation
//! time, description hash, status), and pending ones are listed in a
//! bounded, paginated index so signers can see on-chain what's waiting.
//!
//! The signer set can only change through `propose_signer_change`, which
//! needs the higher `rotation_threshold`, then waits out a mandatory
//! `rotation_delay` during which any single signer can veto. Every
//! executed change bumps the signer-set epoch, which invalidates every
//! outstanding approval.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, xdr::ToXdr, Address, BytesN, Env, Map, Symbol, Val,
    Vec,
};

#[cfg(test)]
mod test;

mod error;
mod events;
mod ttl;
mod types;

use error::Error;
use types::{
    ActionApprovals, ActionMeta, ActionStatus, DataKey, Proposal, SignerChange, SignerWeight,
};

/// Hard cap on the pending-actions index as a whole.
pub const MAX_PENDING_ACTIONS: u32 = 100;
/// Per-signer cap on registered-but-unfinished actions, so one signer
/// can't spam the registry and crowd everyone else out.
pub const MAX_PENDING_PER_SIGNER: u32 = 10;
/// Largest page `get_pending_actions` returns.
pub const MAX_PAGE_LIMIT: u32 = 50;
/// Signers are indexed into a `u32` approval bitmap.
pub const MAX_SIGNERS: u32 = 32;
/// Most actions one `prune` call may name.
pub const MAX_PRUNE: u32 = 20;
/// Bounds a proposal's stored argument list.
pub const MAX_PROPOSAL_ARGS: u32 = 10;
/// Rotation delay the legacy equal-weight `initialize` uses.
pub const DEFAULT_ROTATION_DELAY: u64 = 3 * 24 * 60 * 60;

const DAY_IN_LEDGERS: u32 = 17_280;
const TTL_THRESHOLD: u32 = 7 * DAY_IN_LEDGERS;
const TTL_EXTEND_TO: u32 = 30 * DAY_IN_LEDGERS;

/// Weighted multisig, action registry and proposal executor.
#[contract]
pub struct Multisig;

#[contractimpl]
impl Multisig {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs. Anyone may pay the rent to keep long-lived entries alive.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    /// Deliberately does NOT call require_auth() on any signer, unlike
    /// every other contract's initialize() here requiring its incoming
    /// admin's signature. Naming an address as a signer costs an
    /// attacker nothing without that address's cooperation: approve()
    /// still requires the REAL signer's own signature, so a Multisig
    /// initialized with signers who never consented can simply never
    /// reach is_approved() unless enough of them independently choose
    /// to approve — at which point they could have deployed an honest
    /// instance themselves anyway.
    ///
    /// Backward-compatible equal-weight constructor: every signer gets
    /// weight 1, so `threshold` is a plain M-of-N count. The rotation
    /// threshold is `threshold + 1` (or unanimity when `threshold` is
    /// already every signer) with `DEFAULT_ROTATION_DELAY`.
    ///
    /// `approval_ttl` of zero means approvals never expire; a nonzero
    /// value means an approval older than that many seconds stops
    /// counting toward `is_approved`.
    pub fn initialize(env: Env, signers: Vec<Address>, threshold: u32, approval_ttl: u64) {
        let mut weighted = Vec::new(&env);
        for signer in signers.iter() {
            weighted.push_back((signer, 1u32));
        }
        let rotation_threshold = if threshold >= signers.len() {
            signers.len()
        } else {
            threshold + 1
        };
        Self::initialize_weighted(
            env,
            weighted,
            threshold,
            approval_ttl,
            rotation_threshold,
            DEFAULT_ROTATION_DELAY,
        );
    }

    /// Weighted constructor. Every weight must be above zero, the summed
    /// weight must fit in a `u32` and reach `threshold_weight`, and the
    /// `rotation_threshold` must satisfy the rotation invariant (see
    /// `validate_thresholds`).
    ///
    /// Policy for a signer whose weight alone meets `threshold_weight`:
    /// allowed (e.g. a cold key that can act solo), but announced with a
    /// `single_key_quorum` event so it's visible on-chain.
    pub fn initialize_weighted(
        env: Env,
        signers: Vec<(Address, u32)>,
        threshold_weight: u32,
        approval_ttl: u64,
        rotation_threshold: u32,
        rotation_delay: u64,
    ) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Signers) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        let mut map: Map<Address, u32> = Map::new(&env);
        for (signer, weight) in signers.iter() {
            if weight == 0 {
                panic_with_error!(&env, Error::InvalidWeight);
            }
            if map.contains_key(signer.clone()) {
                panic_with_error!(&env, Error::DuplicateSigner);
            }
            map.set(signer, weight);
        }
        Self::validate_thresholds(&env, &map, threshold_weight, rotation_threshold);

        env.storage().instance().set(&DataKey::Signers, &map);
        env.storage()
            .instance()
            .set(&DataKey::Threshold, &threshold_weight);
        env.storage()
            .instance()
            .set(&DataKey::ApprovalTtl, &approval_ttl);
        env.storage()
            .instance()
            .set(&DataKey::RotationThreshold, &rotation_threshold);
        env.storage()
            .instance()
            .set(&DataKey::RotationDelay, &rotation_delay);
        env.storage().instance().set(&DataKey::SignerEpoch, &0u32);
        Self::warn_single_key_quorum(&env, &map, threshold_weight);
    }

    /// Whether `address` is a current signer. Read-only.
    pub fn is_signer(env: Env, address: Address) -> bool {
        ttl::extend_instance(&env);
        Self::signers(&env).contains_key(address)
    }

    /// Weight of `address`, or zero if it isn't a signer.
    pub fn get_signer_weight(env: Env, address: Address) -> u32 {
        ttl::extend_instance(&env);
        Self::signers(&env).get(address).unwrap_or(0)
    }

    /// Current signers and their weights. Read-only.
    pub fn get_signers(env: Env) -> Map<Address, u32> {
        ttl::extend_instance(&env);
        Self::signers(&env)
    }

    /// Number of signers. Read-only.
    pub fn get_signer_count(env: Env) -> u32 {
        ttl::extend_instance(&env);
        Self::signers(&env).len()
    }

    /// Sum of all signer weights. Read-only.
    pub fn get_total_weight(env: Env) -> u32 {
        ttl::extend_instance(&env);
        Self::total_weight(&env, &Self::signers(&env))
    }

    /// Approval weight `is_approved` requires.
    pub fn get_threshold(env: Env) -> u32 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::Threshold)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant))
    }

    /// Weight needed to rotate the signer set. Read-only.
    pub fn get_rotation_threshold(env: Env) -> u32 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::RotationThreshold)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant))
    }

    /// Delay in seconds between approving and executing a signer change. Read-only.
    pub fn get_rotation_delay(env: Env) -> u64 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::RotationDelay)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant))
    }

    /// Signer-set epoch; bumped on every executed change. Read-only.
    pub fn get_signer_epoch(env: Env) -> u32 {
        ttl::extend_instance(&env);
        Self::epoch(&env)
    }

    /// Seconds an approval stays fresh. Read-only.
    pub fn get_approval_ttl(env: Env) -> u64 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::ApprovalTtl)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant))
    }

    /// Records `signer`'s approval of `action_id`, stamped with the
    /// current ledger timestamp. Requires the signer's own signature and
    /// current membership in the signer set. A signer whose previous
    /// approval of this `action_id` has already expired is treated the
    /// same as one who never approved — they can approve again, which
    /// simply refreshes the timestamp.
    ///
    /// The first approval of an unregistered `action_id` registers it
    /// (proposer = this signer, zero description hash). An action that
    /// was already executed can never be approved again.
    pub fn approve(env: Env, signer: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        signer.require_auth();
        Self::require_signer(&env, &signer);

        let meta = match Self::get_action(env.clone(), action_id.clone()) {
            Some(meta) if meta.status == ActionStatus::Pending => meta,
            Some(meta) if matches!(meta.status, ActionStatus::Executed | ActionStatus::Vetoed) => {
                panic_with_error!(&env, Error::ActionNotPending)
            }
            _ => Self::register(
                &env,
                &signer,
                &action_id,
                &BytesN::from_array(&env, &[0; 32]),
            ),
        };

        let (idx, ttl) = (Self::signer_index(&env, &signer), Self::approval_ttl(&env));
        let now = env.ledger().timestamp();
        let mut approvals = Self::load_approvals(&env, &action_id);
        if Self::is_fresh(&approvals, idx, ttl, now) {
            panic_with_error!(&env, Error::AlreadyApproved);
        }
        approvals.bitmap |= 1 << idx;
        while approvals.timestamps.len() <= idx {
            approvals.timestamps.push_back(0);
        }
        approvals.timestamps.set(idx, now);
        Self::save_approvals(&env, &action_id, &approvals);
        events::approved(&env, signer, action_id, meta.description_hash);
    }

    /// Withdraws `signer`'s own approval of `action_id` — e.g. they
    /// approved before new information came in and want to reconsider.
    /// An already-expired approval has nothing left to withdraw, so this
    /// rejects it the same as a signer who never approved at all.
    pub fn revoke(env: Env, signer: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        signer.require_auth();

        Self::require_signer(&env, &signer);
        let idx = Self::signer_index(&env, &signer);
        let mut approvals = Self::load_approvals(&env, &action_id);
        let (ttl, now) = (Self::approval_ttl(&env), env.ledger().timestamp());
        if !Self::is_fresh(&approvals, idx, ttl, now) {
            panic_with_error!(&env, Error::NotYetApproved);
        }
        approvals.bitmap &= !(1 << idx);
        approvals.timestamps.set(idx, 0);
        Self::save_approvals(&env, &action_id, &approvals);
        let hash = Self::description_hash(&env, &action_id);
        events::revoked(&env, signer, action_id, hash);
    }

    /// True only if `signer` approved `action_id` in the current
    /// signer-set epoch AND that approval hasn't expired under
    /// `approval_ttl` (zero ttl = never expires).
    pub fn has_approved(env: Env, action_id: BytesN<32>, signer: Address) -> bool {
        ttl::extend_instance(&env);
        let Some(idx) = Self::signers(&env).keys().first_index_of(signer) else {
            return false;
        };
        let approvals = Self::load_approvals(&env, &action_id);
        Self::is_fresh(
            &approvals,
            idx,
            Self::approval_ttl(&env),
            env.ledger().timestamp(),
        )
    }

    /// Number of signers with a fresh approval of `action_id`, regardless
    /// of their weight.
    pub fn get_approval_count(env: Env, action_id: BytesN<32>) -> u32 {
        ttl::extend_instance(&env);
        Self::tally(&env, &action_id).0
    }

    /// Summed weight of every fresh approval of `action_id` — recomputed
    /// from each approval's freshness rather than an incremental counter,
    /// since an approval can go stale purely from time passing.
    pub fn get_approval_weight(env: Env, action_id: BytesN<32>) -> u32 {
        ttl::extend_instance(&env);
        Self::tally(&env, &action_id).1
    }

    /// False once the action is vetoed (its approvals are cleared and it
    /// can never be approved again) or its `execute_by` deadline passed.
    pub fn is_approved(env: Env, action_id: BytesN<32>) -> bool {
        ttl::extend_instance(&env);
        let weight = Self::tally(&env, &action_id).1;
        let threshold: u32 = env
            .storage()
            .instance()
            .get(&DataKey::Threshold)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
        // The deadline is only read once quorum is otherwise met, keeping
        // the common path at a single approvals read.
        weight >= threshold && !Self::past_deadline(&env, &action_id)
    }

    /// Clears every signer's approval of `action_id` — for whoever
    /// executed the underlying action to call once it's done, so the
    /// same votes can't linger and be silently reused if `action_id` is
    /// ever reused. Only callable once `action_id` is ALREADY approved,
    /// so it can't be used to grief other signers' pending votes. Also
    /// drops it from the pending index.
    pub fn reset(env: Env, action_id: BytesN<32>) {
        if !Self::is_approved(env.clone(), action_id.clone()) {
            panic_with_error!(&env, Error::NotYetApproved);
        }
        let storage = env.storage().persistent();
        if storage.has(&DataKey::Proposal(action_id.clone()))
            || storage.has(&DataKey::SignerChange(action_id.clone()))
        {
            // Proposals and signer changes leave the registry only
            // through their own execute/veto paths.
            panic_with_error!(&env, Error::ActionNotPending);
        }
        Self::clear_approvals(&env, &action_id);
        let hash = Self::finish(&env, &action_id, ActionStatus::Reset);
        events::reset(&env, action_id, hash);
    }

    // ── Registry ──────────────────────────────────────────────────────────────

    /// Explicitly registers `action_id` with a `description_hash` (e.g.
    /// sha256 of an off-chain write-up) before anyone votes on it, so
    /// signers can check on-chain what they're approving.
    pub fn register_action(
        env: Env,
        proposer: Address,
        action_id: BytesN<32>,
        description_hash: BytesN<32>,
    ) {
        proposer.require_auth();
        Self::require_signer(&env, &proposer);
        if let Some(meta) = Self::get_action(env.clone(), action_id.clone()) {
            if matches!(
                meta.status,
                ActionStatus::Pending | ActionStatus::Executed | ActionStatus::Vetoed
            ) {
                panic_with_error!(&env, Error::ActionAlreadyRegistered);
            }
        }
        Self::register(&env, &proposer, &action_id, &description_hash);
    }

    /// Drops a pending action from the index once it has gone stale: an
    /// `approval_ttl` is configured, the action is older than it, and no
    /// approval of it is still fresh. Callable by anyone.
    pub fn expire(env: Env, action_id: BytesN<32>) {
        let meta = Self::get_action(env.clone(), action_id.clone())
            .unwrap_or_else(|| panic_with_error!(&env, Error::ActionNotPending));
        if meta.status != ActionStatus::Pending {
            panic_with_error!(&env, Error::ActionNotPending);
        }
        if Self::past_deadline(&env, &action_id) {
            let execute_by = Self::get_execute_by(env.clone(), action_id.clone())
                .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
            Self::clear_approvals(&env, &action_id);
            Self::finish(&env, &action_id, ActionStatus::Expired);
            events::action_expired(&env, action_id, execute_by);
            return;
        }
        let ttl: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ApprovalTtl)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
        let age = env.ledger().timestamp().saturating_sub(meta.created_at);
        if ttl == 0 || age <= ttl || Self::get_approval_count(env.clone(), action_id.clone()) > 0 {
            panic_with_error!(&env, Error::NotExpired);
        }
        Self::clear_approvals(&env, &action_id);
        let hash = Self::finish(&env, &action_id, ActionStatus::Expired);
        events::expired(&env, action_id, hash);
    }

    /// Sets the vetoer exactly once. It can only be called by this
    /// contract itself, i.e. through an approved `propose`/`execute`, and
    /// is immutable afterwards (rotating a compromised vetoer means
    /// redeploying), so a compromised signer majority can't swap it out.
    pub fn set_vetoer(env: Env, vetoer: Address) {
        ttl::extend_instance(&env);
        env.current_contract_address().require_auth();
        if env.storage().instance().has(&DataKey::Vetoer) {
            panic_with_error!(&env, Error::VetoerAlreadySet);
        }
        env.storage().instance().set(&DataKey::Vetoer, &vetoer);
    }

    /// Signer allowed to veto pending signer changes, if set. Read-only.
    pub fn get_vetoer(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Vetoer)
    }

    /// Cancels a pending action for good: its approvals are cleared and
    /// `is_approved` stays false. Vetoing an already-executed (or
    /// otherwise finished) action is rejected. The vetoer may also be a
    /// signer.
    pub fn veto(env: Env, vetoer: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        vetoer.require_auth();
        let stored: Option<Address> = env.storage().instance().get(&DataKey::Vetoer);
        if stored != Some(vetoer.clone()) {
            panic_with_error!(&env, Error::NotVetoer);
        }
        Self::require_pending(&env, &action_id);
        Self::clear_approvals(&env, &action_id);
        let hash = Self::finish(&env, &action_id, ActionStatus::Vetoed);
        events::vetoed(&env, vetoer, action_id, hash);
    }

    /// Sets a hard deadline (ledger timestamp) after which `is_approved`
    /// is false for this pending action, regardless of `approval_ttl`.
    /// Only the action's proposer may set it, once, and it must lie in
    /// the future. The boundary is inclusive: at `execute_by` the action
    /// is still approved.
    pub fn set_execute_by(env: Env, proposer: Address, action_id: BytesN<32>, execute_by: u64) {
        ttl::extend_instance(&env);
        proposer.require_auth();
        let meta = Self::get_action(env.clone(), action_id.clone())
            .unwrap_or_else(|| panic_with_error!(&env, Error::ActionNotPending));
        if meta.status != ActionStatus::Pending {
            panic_with_error!(&env, Error::ActionNotPending);
        }
        if meta.proposer != proposer {
            panic_with_error!(&env, Error::NotProposer);
        }
        if execute_by <= env.ledger().timestamp() {
            panic_with_error!(&env, Error::InvalidDeadline);
        }
        let key = DataKey::ExecuteBy(action_id);
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, Error::DeadlineAlreadySet);
        }
        ttl::set_persistent(&env, &key, &execute_by);
    }

    /// Execution deadline of `action_id`, if one was set. Read-only.
    pub fn get_execute_by(env: Env, action_id: BytesN<32>) -> Option<u64> {
        ttl::get_persistent(&env, &DataKey::ExecuteBy(action_id))
    }

    /// Permissionless, bounded cleanup: removes the approvals entry of every
    /// named action whose entry can no longer affect `is_approved` — it is
    /// finished (executed, reset, expired, vetoed) or holds no fresh
    /// approval and has no passed deadline. Actions that don't qualify are
    /// skipped. Returns how many entries were removed. Action registry
    /// entries (the executed flag) and deadlines are never pruned.
    pub fn prune(env: Env, action_ids: Vec<BytesN<32>>) -> u32 {
        ttl::extend_instance(&env);
        if action_ids.len() > MAX_PRUNE {
            panic_with_error!(&env, Error::InvalidPageLimit);
        }
        let mut removed = 0u32;
        for id in action_ids.iter() {
            if !Self::approvals_exist(&env, &id) {
                continue;
            }
            let finished = Self::get_action(env.clone(), id.clone())
                .map(|m| m.status != ActionStatus::Pending)
                .unwrap_or(true);
            let dead = Self::tally(&env, &id).0 == 0 && !Self::past_deadline(&env, &id);
            if finished || dead {
                Self::clear_approvals(&env, &id);
                removed += 1;
            }
        }
        removed
    }

    /// Registry entry for `action_id`, if any. Read-only.
    pub fn get_action(env: Env, action_id: BytesN<32>) -> Option<ActionMeta> {
        env.storage().persistent().get(&DataKey::Action(action_id))
    }

    /// Number of pending actions. Read-only.
    pub fn get_pending_count(env: Env) -> u32 {
        Self::pending(&env).len()
    }

    /// Up to `limit` (≤ MAX_PAGE_LIMIT) pending action ids starting at
    /// index `cursor`. Removal is swap-remove, so order isn't stable
    /// across writes.
    pub fn get_pending_actions(env: Env, cursor: u32, limit: u32) -> Vec<BytesN<32>> {
        if limit == 0 || limit > MAX_PAGE_LIMIT {
            panic_with_error!(&env, Error::InvalidPageLimit);
        }
        let pending = Self::pending(&env);
        let mut page = Vec::new(&env);
        let end = cursor.saturating_add(limit).min(pending.len());
        for i in cursor..end {
            page.push_back(
                pending
                    .get(i)
                    .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant)),
            );
        }
        page
    }

    // ── Executor ──────────────────────────────────────────────────────────────

    /// Registers a call the multisig will make as itself once approved:
    /// `target.function(args)`. The returned id is the sha256 of the
    /// proposal's XDR (including a fresh nonce) and is also its
    /// description hash, so the id commits to the exact payload.
    pub fn propose(
        env: Env,
        proposer: Address,
        target: Address,
        function: Symbol,
        args: Vec<Val>,
    ) -> BytesN<32> {
        proposer.require_auth();
        Self::require_signer(&env, &proposer);
        if args.len() > MAX_PROPOSAL_ARGS {
            panic_with_error!(&env, Error::ProposalTooLarge);
        }

        let proposal = Proposal {
            target,
            function,
            args,
            nonce: Self::next_nonce(&env),
        };
        let id: BytesN<32> = env.crypto().sha256(&proposal.clone().to_xdr(&env)).into();
        let key = DataKey::Proposal(id.clone());
        env.storage().persistent().set(&key, &proposal);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::register(&env, &proposer, &id, &id);
        id
    }

    /// Runs an approved proposal: `target.function(args)` invoked by this
    /// contract, which satisfies a target's `admin.require_auth()` when
    /// the multisig is that admin. Callable by anyone. Marked executed
    /// before the call, so it can never run twice; a failing target call
    /// reverts the whole transaction, including that mark.
    pub fn execute(env: Env, proposal_id: BytesN<32>) -> Val {
        let proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id.clone()))
            .unwrap_or_else(|| panic_with_error!(&env, Error::ProposalNotFound));
        Self::require_pending(&env, &proposal_id);
        if !Self::is_approved(env.clone(), proposal_id.clone()) {
            panic_with_error!(&env, Error::NotYetApproved);
        }

        Self::clear_approvals(&env, &proposal_id);
        let hash = Self::finish(&env, &proposal_id, ActionStatus::Executed);
        events::executed(&env, proposal_id, hash);

        env.invoke_contract::<Val>(&proposal.target, &proposal.function, proposal.args)
    }

    /// Proposal by id, if it exists. Read-only.
    pub fn get_proposal(env: Env, proposal_id: BytesN<32>) -> Option<Proposal> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
    }

    // ── Signer rotation ───────────────────────────────────────────────────────

    /// Proposes adding at most one signer and removing at most one (each
    /// list holds zero or one entries), plus
    /// the new approval and rotation thresholds. Passing the same address
    /// as both `add` and `remove` changes that signer's weight. The
    /// returned id is the sha256 of the change's XDR, so approvals bind
    /// to the exact payload. Validated now against the resulting set,
    /// and again at execution.
    ///
    /// Lifecycle: signers `approve(id)` until the approval weight reaches
    /// `rotation_threshold`; anyone calls `queue_signer_change`, starting
    /// the `rotation_delay`; once it elapses anyone calls
    /// `execute_signer_change`. Any single signer can
    /// `veto_signer_change` until it executes.
    pub fn propose_signer_change(
        env: Env,
        proposer: Address,
        add: Vec<SignerWeight>,
        remove: Vec<Address>,
        new_threshold: u32,
        new_rotation_threshold: u32,
    ) -> BytesN<32> {
        ttl::extend_instance(&env);
        proposer.require_auth();
        Self::require_signer(&env, &proposer);

        let change = SignerChange {
            add,
            remove,
            new_threshold,
            new_rotation_threshold,
            epoch: Self::epoch(&env),
            nonce: Self::next_nonce(&env),
        };
        Self::apply_change(&env, &change);

        let id: BytesN<32> = env.crypto().sha256(&change.clone().to_xdr(&env)).into();
        ttl::set_persistent(&env, &DataKey::SignerChange(id.clone()), &change);
        Self::register(&env, &proposer, &id, &id);
        events::signer_change_proposed(&env, proposer, id.clone(), change);
        id
    }

    /// Starts the mandatory delay once `change_id`'s approval weight has
    /// reached `rotation_threshold`. Callable by anyone.
    pub fn queue_signer_change(env: Env, change_id: BytesN<32>) -> u64 {
        ttl::extend_instance(&env);
        Self::require_live_change(&env, &change_id);
        let ready_key = DataKey::SignerChangeReadyAt(change_id.clone());
        if env.storage().persistent().has(&ready_key) {
            panic_with_error!(&env, Error::ActionAlreadyRegistered);
        }
        Self::require_rotation_approved(&env, &change_id);
        let delay: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RotationDelay)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
        let ready_at = env.ledger().timestamp().saturating_add(delay);
        ttl::set_persistent(&env, &ready_key, &ready_at);
        events::signer_change_queued(&env, change_id, ready_at);
        ready_at
    }

    /// Any single signer can kill a pending signer change, before or
    /// during its delay.
    pub fn veto_signer_change(env: Env, signer: Address, change_id: BytesN<32>) {
        ttl::extend_instance(&env);
        signer.require_auth();
        Self::require_signer(&env, &signer);
        Self::require_live_change(&env, &change_id);
        Self::clear_approvals(&env, &change_id);
        Self::finish(&env, &change_id, ActionStatus::Reset);
        events::signer_change_vetoed(&env, signer, change_id);
    }

    /// Applies a queued signer change once its delay has elapsed, as long
    /// as its approval weight still meets `rotation_threshold`. Bumps the
    /// signer-set epoch, which invalidates every outstanding approval
    /// (including those of a removed signer). Callable by anyone.
    pub fn execute_signer_change(env: Env, change_id: BytesN<32>) {
        ttl::extend_instance(&env);
        let change = Self::require_live_change(&env, &change_id);
        let ready_at: u64 =
            ttl::get_persistent(&env, &DataKey::SignerChangeReadyAt(change_id.clone()))
                .unwrap_or_else(|| panic_with_error!(&env, Error::SignerChangeNotQueued));
        if env.ledger().timestamp() < ready_at {
            panic_with_error!(&env, Error::RotationDelayActive);
        }
        Self::require_rotation_approved(&env, &change_id);

        let signers = Self::apply_change(&env, &change);
        Self::clear_approvals(&env, &change_id);
        Self::finish(&env, &change_id, ActionStatus::Executed);

        let epoch = change.epoch + 1;
        let storage = env.storage().instance();
        storage.set(&DataKey::Signers, &signers);
        storage.set(&DataKey::Threshold, &change.new_threshold);
        storage.set(&DataKey::RotationThreshold, &change.new_rotation_threshold);
        storage.set(&DataKey::SignerEpoch, &epoch);
        Self::warn_single_key_quorum(&env, &signers, change.new_threshold);
        events::signers_rotated(&env, change_id, epoch);
    }

    /// Pending signer change by id, if it exists. Read-only.
    pub fn get_signer_change(env: Env, change_id: BytesN<32>) -> Option<SignerChange> {
        ttl::get_persistent(&env, &DataKey::SignerChange(change_id))
    }

    /// Timestamp at which a queued change becomes executable.
    pub fn get_signer_change_ready_at(env: Env, change_id: BytesN<32>) -> Option<u64> {
        ttl::get_persistent(&env, &DataKey::SignerChangeReadyAt(change_id))
    }
}

impl Multisig {
    fn signers(env: &Env) -> Map<Address, u32> {
        env.storage()
            .instance()
            .get(&DataKey::Signers)
            .unwrap_or_else(|| panic_with_error!(env, Error::InternalInvariant))
    }

    fn epoch(env: &Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::SignerEpoch)
            .unwrap_or(0)
    }

    fn next_nonce(env: &Env) -> u64 {
        let nonce: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ProposalNonce)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::ProposalNonce, &(nonce + 1));
        nonce
    }

    /// Overflow-safe sum of every signer's weight.
    fn total_weight(env: &Env, signers: &Map<Address, u32>) -> u32 {
        let mut total = 0u32;
        for (_, weight) in signers.iter() {
            total = total
                .checked_add(weight)
                .unwrap_or_else(|| panic_with_error!(env, Error::WeightOverflow));
        }
        total
    }

    /// Invariants for any signer set: non-empty, `1 <= threshold <=
    /// total weight`, and a rotation threshold strictly above
    /// `threshold` — or, when `threshold` is already the total weight,
    /// equal to it (unanimity) — that is still reachable.
    fn validate_thresholds(
        env: &Env,
        signers: &Map<Address, u32>,
        threshold: u32,
        rotation_threshold: u32,
    ) {
        let total = Self::total_weight(env, signers);
        if signers.len() > MAX_SIGNERS {
            panic_with_error!(env, Error::TooManySigners);
        }
        if signers.is_empty() || threshold == 0 || threshold > total {
            panic_with_error!(env, Error::InvalidThreshold);
        }
        let above = rotation_threshold > threshold;
        let unanimous = rotation_threshold == total;
        if rotation_threshold > total || !(above || unanimous) {
            panic_with_error!(env, Error::InvalidThreshold);
        }
    }

    /// Returns the signer set `change` would produce, panicking on any
    /// invariant violation.
    fn apply_change(env: &Env, change: &SignerChange) -> Map<Address, u32> {
        if change.add.len() > 1 || change.remove.len() > 1 {
            panic_with_error!(env, Error::InvalidSignerChange);
        }
        let mut signers = Self::signers(env);
        if let Some(remove) = change.remove.first() {
            if !signers.contains_key(remove.clone()) {
                panic_with_error!(env, Error::NotASigner);
            }
            signers.remove(remove);
        }
        if let Some(add) = change.add.first() {
            if add.weight == 0 {
                panic_with_error!(env, Error::InvalidWeight);
            }
            if signers.contains_key(add.address.clone()) {
                panic_with_error!(env, Error::DuplicateSigner);
            }
            signers.set(add.address, add.weight);
        }
        Self::validate_thresholds(
            env,
            &signers,
            change.new_threshold,
            change.new_rotation_threshold,
        );
        signers
    }

    fn require_live_change(env: &Env, change_id: &BytesN<32>) -> SignerChange {
        let change: SignerChange =
            ttl::get_persistent(env, &DataKey::SignerChange(change_id.clone()))
                .unwrap_or_else(|| panic_with_error!(env, Error::SignerChangeNotFound));
        Self::require_pending(env, change_id);
        if change.epoch != Self::epoch(env) {
            panic_with_error!(env, Error::StaleSignerChange);
        }
        change
    }

    fn require_rotation_approved(env: &Env, change_id: &BytesN<32>) {
        let weight = Self::get_approval_weight(env.clone(), change_id.clone());
        let needed: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RotationThreshold)
            .unwrap_or_else(|| panic_with_error!(env, Error::InternalInvariant));
        if weight < needed {
            panic_with_error!(env, Error::NotYetApproved);
        }
    }

    fn warn_single_key_quorum(env: &Env, signers: &Map<Address, u32>, threshold: u32) {
        if signers.len() < 2 {
            return;
        }
        for (signer, weight) in signers.iter() {
            if weight >= threshold {
                events::single_key_quorum(env, signer, weight, threshold);
            }
        }
    }

    fn require_signer(env: &Env, address: &Address) {
        if !Self::signers(env).contains_key(address.clone()) {
            panic_with_error!(env, Error::NotASigner);
        }
    }

    fn require_pending(env: &Env, action_id: &BytesN<32>) {
        let pending = Self::get_action(env.clone(), action_id.clone())
            .map(|m| m.status == ActionStatus::Pending)
            .unwrap_or(false);
        if !pending {
            panic_with_error!(env, Error::ActionNotPending);
        }
    }

    fn pending(env: &Env) -> Vec<BytesN<32>> {
        env.storage()
            .persistent()
            .get(&DataKey::Pending)
            .unwrap_or_else(|| Vec::new(env))
    }

    fn description_hash(env: &Env, action_id: &BytesN<32>) -> BytesN<32> {
        Self::get_action(env.clone(), action_id.clone())
            .map(|m| m.description_hash)
            .unwrap_or_else(|| BytesN::from_array(env, &[0; 32]))
    }

    fn register(
        env: &Env,
        proposer: &Address,
        action_id: &BytesN<32>,
        description_hash: &BytesN<32>,
    ) -> ActionMeta {
        let mut pending = Self::pending(env);
        if pending.len() >= MAX_PENDING_ACTIONS {
            panic_with_error!(env, Error::TooManyPendingActions);
        }
        let count_key = DataKey::PendingCount(proposer.clone());
        let count: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);
        if count >= MAX_PENDING_PER_SIGNER {
            panic_with_error!(env, Error::TooManyPendingActions);
        }
        env.storage().persistent().set(&count_key, &(count + 1));
        pending.push_back(action_id.clone());
        env.storage().persistent().set(&DataKey::Pending, &pending);

        let meta = ActionMeta {
            proposer: proposer.clone(),
            created_at: env.ledger().timestamp(),
            description_hash: description_hash.clone(),
            status: ActionStatus::Pending,
        };
        let key = DataKey::Action(action_id.clone());
        env.storage().persistent().set(&key, &meta);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
        events::registered(
            env,
            proposer.clone(),
            action_id.clone(),
            description_hash.clone(),
        );
        meta
    }

    /// Moves a pending action to its final `status`, swap-removing it
    /// from the index. Returns its description hash for events.
    fn finish(env: &Env, action_id: &BytesN<32>, status: ActionStatus) -> BytesN<32> {
        let Some(mut meta) = Self::get_action(env.clone(), action_id.clone()) else {
            return BytesN::from_array(env, &[0; 32]);
        };
        if meta.status == ActionStatus::Pending {
            let mut pending = Self::pending(env);
            if let Some(i) = pending.first_index_of(action_id) {
                let last = pending.len() - 1;
                if i != last {
                    pending.set(
                        i,
                        pending
                            .get(last)
                            .unwrap_or_else(|| panic_with_error!(env, Error::InternalInvariant)),
                    );
                }
                pending.pop_back();
                env.storage().persistent().set(&DataKey::Pending, &pending);
            }
            let count_key = DataKey::PendingCount(meta.proposer.clone());
            let count: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);
            env.storage()
                .persistent()
                .set(&count_key, &count.saturating_sub(1));
        }
        meta.status = status;
        // Executed-flag retention policy: the finished entry is the replay
        // protection, so it is never pruned and is kept alive with the
        // standard persistent TTL (renewable by anyone through `bump`).
        ttl::set_persistent(env, &DataKey::Action(action_id.clone()), &meta);
        meta.description_hash
    }

    fn approval_ttl(env: &Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::ApprovalTtl)
            .unwrap_or_else(|| panic_with_error!(env, Error::InternalInvariant))
    }

    fn signer_index(env: &Env, signer: &Address) -> u32 {
        Self::signers(env)
            .keys()
            .first_index_of(signer.clone())
            .unwrap_or_else(|| panic_with_error!(env, Error::NotASigner))
    }

    fn is_fresh(approvals: &ActionApprovals, idx: u32, ttl: u64, now: u64) -> bool {
        if approvals.bitmap & (1 << idx) == 0 {
            return false;
        }
        let at = approvals.timestamps.get(idx).unwrap_or(0);
        ttl == 0 || now.checked_sub(at).unwrap_or(u64::MAX) <= ttl
    }

    /// (fresh approval count, fresh approval weight) from the one entry.
    fn tally(env: &Env, action_id: &BytesN<32>) -> (u32, u32) {
        let approvals = Self::load_approvals(env, action_id);
        if approvals.bitmap == 0 {
            return (0, 0);
        }
        let (ttl, now) = (Self::approval_ttl(env), env.ledger().timestamp());
        let (mut count, mut weight) = (0u32, 0u32);
        for (i, (_, w)) in Self::signers(env).iter().enumerate() {
            if Self::is_fresh(&approvals, i as u32, ttl, now) {
                count += 1;
                // Bounded by the total weight, which initialize and every
                // rotation checked fits in a u32.
                weight += w;
            }
        }
        (count, weight)
    }

    fn past_deadline(env: &Env, action_id: &BytesN<32>) -> bool {
        let at: Option<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::ExecuteBy(action_id.clone()));
        at.map(|t| env.ledger().timestamp() > t).unwrap_or(false)
    }

    // Storage class: with `approval_ttl > 0` an approvals entry can only
    // matter while its newest approval is fresh, so it lives in temporary
    // storage with a TTL covering `approval_ttl` (conservatively assuming
    // ledgers close at least every 3s): when it lapses, every approval in it
    // has already expired, which is exactly "no approvals". With
    // `approval_ttl == 0` approvals never expire, so they stay persistent.
    fn approvals_key(env: &Env, action_id: &BytesN<32>) -> DataKey {
        DataKey::Approvals(Self::epoch(env), action_id.clone())
    }

    fn approvals_exist(env: &Env, action_id: &BytesN<32>) -> bool {
        let key = Self::approvals_key(env, action_id);
        if Self::approval_ttl(env) > 0 {
            env.storage().temporary().has(&key)
        } else {
            env.storage().persistent().has(&key)
        }
    }

    fn load_approvals(env: &Env, action_id: &BytesN<32>) -> ActionApprovals {
        let key = Self::approvals_key(env, action_id);
        let found: Option<ActionApprovals> = if Self::approval_ttl(env) > 0 {
            env.storage().temporary().get(&key)
        } else {
            ttl::get_persistent(env, &key)
        };
        found.unwrap_or(ActionApprovals {
            bitmap: 0,
            timestamps: Vec::new(env),
        })
    }

    fn save_approvals(env: &Env, action_id: &BytesN<32>, approvals: &ActionApprovals) {
        if approvals.bitmap == 0 {
            return Self::clear_approvals(env, action_id);
        }
        let key = Self::approvals_key(env, action_id);
        let ttl = Self::approval_ttl(env);
        if ttl > 0 {
            let storage = env.storage().temporary();
            storage.set(&key, approvals);
            let ledgers = (ttl / 3 + 1).min(u32::MAX as u64) as u32;
            let ledgers = ledgers.min(env.storage().max_ttl());
            storage.extend_ttl(&key, ledgers, ledgers);
        } else {
            ttl::set_persistent(env, &key, approvals);
        }
    }

    fn clear_approvals(env: &Env, action_id: &BytesN<32>) {
        let key = Self::approvals_key(env, action_id);
        if Self::approval_ttl(env) > 0 {
            env.storage().temporary().remove(&key);
        } else {
            env.storage().persistent().remove(&key);
        }
    }
}
