#![no_std]

//! Zenith Timelock — delayed, cancellable execution of admin actions.
//!
//! The `proposer` queues a `Call` with an optional `predecessor`; after
//! `delay` seconds the `executor` runs it. The `canceller` (or the
//! `guardian`, if wired) can cancel while it is pending.
//!
//! **Queries.** Every view is unauthenticated, reads with plain storage
//! gets, and never writes or extends a TTL. Enumeration is bounded: the
//! pending set is an id list capped at `MAX_PENDING`, and `get_pending`
//! serves at most `MAX_PAGE` ids per call, ascending by id. Ids are
//! assigned from a counter and are never reused, but the pending list is
//! not contiguous once proposals execute or are cancelled, so `offset`
//! indexes into the *pending list*, not into the id space.

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, Val, Vec};

#[cfg(test)]
mod test;

mod error;
mod events;
mod types;

pub use error::Error;
pub use types::{Call, DataKey, Proposal, ProposalStatus};

/// Hard cap on simultaneously pending proposals.
pub const MAX_PENDING: u32 = 100;
/// Hard cap on `get_pending`'s `limit`.
pub const MAX_PAGE: u32 = 50;

const DAY_IN_LEDGERS: u32 = 17_280;
const BUMP: u32 = 90 * DAY_IN_LEDGERS;
const THRESHOLD: u32 = BUMP - 30 * DAY_IN_LEDGERS;

#[contract]
pub struct Timelock;

#[contractimpl]
impl Timelock {
    pub fn initialize(
        env: Env,
        proposer: Address,
        executor: Address,
        canceller: Address,
        delay: u64,
        guardian: Option<Address>,
    ) {
        if env.storage().instance().has(&DataKey::Delay) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        let s = env.storage().instance();
        s.set(&DataKey::Proposer, &proposer);
        s.set(&DataKey::Executor, &executor);
        s.set(&DataKey::Canceller, &canceller);
        s.set(&DataKey::Delay, &delay);
        if let Some(g) = guardian {
            s.set(&DataKey::Guardian, &g);
        }
        s.set(&DataKey::NextId, &0u64);
        s.extend_ttl(THRESHOLD, BUMP);
    }

    // ── State machine ─────────────────────────────────────────────────────────

    /// Queues `call`, executable `delay` seconds from now. A predecessor
    /// must already exist and not be cancelled. Returns the new id.
    pub fn propose(env: Env, call: Call, predecessor: Option<u64>) -> u64 {
        env.storage().instance().extend_ttl(THRESHOLD, BUMP);
        let proposer = Self::proposer(env.clone());
        proposer.require_auth();
        if let Some(p) = predecessor {
            match proposal(&env, p) {
                Some(prev) if prev.status != ProposalStatus::Cancelled => {}
                _ => panic_with_error!(&env, Error::InvalidPredecessor),
            }
        }
        let mut pending = pending(&env);
        if pending.len() >= MAX_PENDING {
            panic_with_error!(&env, Error::TooManyPending);
        }
        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap();
        env.storage().instance().set(&DataKey::NextId, &(id + 1));
        let execute_after = env.ledger().timestamp() + Self::delay(env.clone());
        save(
            &env,
            &Proposal {
                id,
                call,
                predecessor,
                proposer: proposer.clone(),
                execute_after,
                status: ProposalStatus::Pending,
            },
        );
        pending.push_back(id);
        save_pending(&env, &pending);
        events::proposed(&env, id, proposer, execute_after);
        id
    }

    /// Runs a ready proposal. Marked executed before the call, so a
    /// reentrant `execute` of the same id fails.
    pub fn execute(env: Env, id: u64) -> Val {
        env.storage().instance().extend_ttl(THRESHOLD, BUMP);
        Self::executor(env.clone()).require_auth();
        let mut p =
            proposal(&env, id).unwrap_or_else(|| panic_with_error!(&env, Error::ProposalNotFound));
        if p.status != ProposalStatus::Pending {
            panic_with_error!(&env, Error::NotPending);
        }
        if env.ledger().timestamp() < p.execute_after {
            panic_with_error!(&env, Error::NotReady);
        }
        if !predecessor_done(&env, &p) {
            panic_with_error!(&env, Error::PredecessorNotExecuted);
        }
        p.status = ProposalStatus::Executed;
        save(&env, &p);
        remove_pending(&env, id);
        events::executed(&env, id);
        env.invoke_contract(&p.call.target, &p.call.func, p.call.args)
    }

    /// Cancels a pending proposal; callable by the canceller or guardian.
    pub fn cancel(env: Env, caller: Address, id: u64) {
        env.storage().instance().extend_ttl(THRESHOLD, BUMP);
        let is_canceller = caller == Self::canceller(env.clone());
        if !is_canceller && Some(&caller) != Self::guardian(env.clone()).as_ref() {
            panic_with_error!(&env, Error::Unauthorized);
        }
        caller.require_auth();
        let mut p =
            proposal(&env, id).unwrap_or_else(|| panic_with_error!(&env, Error::ProposalNotFound));
        if p.status != ProposalStatus::Pending {
            panic_with_error!(&env, Error::NotPending);
        }
        p.status = ProposalStatus::Cancelled;
        save(&env, &p);
        remove_pending(&env, id);
        events::cancelled(&env, id);
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    pub fn get_proposal(env: Env, id: u64) -> Option<Proposal> {
        proposal(&env, id)
    }

    /// Pending, past `execute_after`, and its predecessor (if any) executed.
    pub fn is_executable(env: Env, id: u64) -> bool {
        match proposal(&env, id) {
            Some(p) => {
                p.status == ProposalStatus::Pending
                    && env.ledger().timestamp() >= p.execute_after
                    && predecessor_done(&env, &p)
            }
            None => false,
        }
    }

    pub fn is_executed(env: Env, id: u64) -> bool {
        status_is(&env, id, ProposalStatus::Executed)
    }

    pub fn is_cancelled(env: Env, id: u64) -> bool {
        status_is(&env, id, ProposalStatus::Cancelled)
    }

    /// When a pending proposal's delay elapses; `None` once it is executed,
    /// cancelled, or unknown. It does not account for the predecessor.
    pub fn next_executable_at(env: Env, id: u64) -> Option<u64> {
        proposal(&env, id)
            .filter(|p| p.status == ProposalStatus::Pending)
            .map(|p| p.execute_after)
    }

    pub fn predecessor(env: Env, id: u64) -> Option<u64> {
        proposal(&env, id).and_then(|p| p.predecessor)
    }

    /// Total proposals ever created (the next id).
    pub fn proposal_count(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::NextId).unwrap_or(0)
    }

    /// Up to `limit` (1..=`MAX_PAGE`) pending ids starting at `offset` in
    /// the pending list; an `offset` past the end yields an empty page.
    pub fn get_pending(env: Env, offset: u32, limit: u32) -> Vec<u64> {
        if limit == 0 || limit > MAX_PAGE {
            panic_with_error!(&env, Error::InvalidPagination);
        }
        let all = pending(&env);
        let end = offset.saturating_add(limit).min(all.len());
        if offset >= end {
            return Vec::new(&env);
        }
        all.slice(offset..end)
    }

    pub fn pending_count(env: Env) -> u32 {
        pending(&env).len()
    }

    pub fn proposer(env: Env) -> Address {
        role(&env, &DataKey::Proposer)
    }

    pub fn executor(env: Env) -> Address {
        role(&env, &DataKey::Executor)
    }

    pub fn canceller(env: Env) -> Address {
        role(&env, &DataKey::Canceller)
    }

    pub fn delay(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::Delay)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotInitialized))
    }

    pub fn guardian(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Guardian)
    }
}

fn role(env: &Env, key: &DataKey) -> Address {
    env.storage()
        .instance()
        .get(key)
        .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized))
}

fn proposal(env: &Env, id: u64) -> Option<Proposal> {
    env.storage().persistent().get(&DataKey::Proposal(id))
}

fn status_is(env: &Env, id: u64, status: ProposalStatus) -> bool {
    proposal(env, id).map_or(false, |p| p.status == status)
}

fn predecessor_done(env: &Env, p: &Proposal) -> bool {
    match p.predecessor {
        Some(prev) => status_is(env, prev, ProposalStatus::Executed),
        None => true,
    }
}

fn save(env: &Env, p: &Proposal) {
    let key = DataKey::Proposal(p.id);
    env.storage().persistent().set(&key, p);
    env.storage().persistent().extend_ttl(&key, THRESHOLD, BUMP);
}

fn pending(env: &Env) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::Pending)
        .unwrap_or_else(|| Vec::new(env))
}

fn save_pending(env: &Env, list: &Vec<u64>) {
    env.storage().persistent().set(&DataKey::Pending, list);
    env.storage()
        .persistent()
        .extend_ttl(&DataKey::Pending, THRESHOLD, BUMP);
}

fn remove_pending(env: &Env, id: u64) {
    let mut list = pending(env);
    if let Some(i) = list.first_index_of(id) {
        list.remove(i);
        save_pending(env, &list);
    }
}
