#![no_std]

//! Zenith Timelock — delays every privileged call by at least `min_delay`
//! so users can react (or a canceller can abort) before it lands.
//!
//! Roles:
//!
//! - **Proposer** (one address): schedules batches of contract calls.
//! - **Executor**: anyone by default (an "open executor", as in
//!   OpenZeppelin's TimelockController), or one address once
//!   `set_exec(Some(addr))` runs. Open execution is what lets a governor
//!   proposal target the governor itself: the governor never sits in the
//!   call stack, so Soroban's no-re-entry rule is not tripped.
//! - **Cancellers**: the proposer, the guardian (until `lock_in`), and any
//!   address granted with a `set_canc(addr, true)` self-call. Any of them
//!   can abort a pending operation.
//! - **Guardian**: also reassigns the proposer immediately (the rollback
//!   path of the governance handover, docs/governance-handover.md).
//!   `lock_in` removes it for good.
//!
//! Contracts that hand their admin to this timelock accept its calls
//! because the timelock is the direct invoker, so `admin.require_auth()`
//! passes without any signature. Those contracts stay agnostic about
//! whether their admin is a key or a contract.
//!
//! Operations may name a **predecessor** (`schedule_after`): an operation
//! that must have executed first. The predecessor has to exist when the
//! dependent is scheduled, so dependencies always point back in time and
//! can never form a cycle; the worst a cancelled predecessor can do is
//! leave its dependents unexecutable until they're cancelled too.
//!
//! Soroban forbids a contract re-entering itself, so the timelock's own
//! configuration is changed by scheduling an operation whose target is
//! the timelock itself; `execute` recognises those and applies them in
//! place instead of cross-calling: `set_prop(Address)`, `set_delay(u64)`,
//! `set_canc(Address, bool)`, `set_exec(Option<Address>)`, `lock_in()`,
//! `migrate(u32, u32)`.
//!
//! Every delay is measured on the ledger timestamp, the same clock the
//! governor uses for its proposal `eta`.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, symbol_short, Address, BytesN, Env, Symbol,
    TryFromVal, Val, Vec,
};

#[cfg(test)]
mod test;

mod error;
mod events;
mod migrations;
mod types;

use error::Error;
use types::DataKey;
pub use types::Operation;

// Instance and operation entries are kept alive for at least ~30 days
// past their last touch, and bumped back to ~60 days whenever they get
// close — longer than any sensible min_delay + grace_period.
const DAY_IN_LEDGERS: u32 = 17_280;
const BUMP_THRESHOLD: u32 = 30 * DAY_IN_LEDGERS;
const BUMP_AMOUNT: u32 = 60 * DAY_IN_LEDGERS;

fn extend_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(BUMP_THRESHOLD, BUMP_AMOUNT);
}

#[contract]
pub struct Timelock;

fn proposer(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::Proposer)
        .unwrap_or_else(|| panic_with_error!(env, Error::Unauthorized))
}

fn set_proposer(env: &Env, new: Address) {
    events::proposer_set(env, proposer(env), new.clone());
    env.storage().instance().set(&DataKey::Proposer, &new);
}

fn get_op(env: &Env, op_id: &BytesN<32>) -> Option<Operation> {
    let key = DataKey::Operation(op_id.clone());
    let op = env.storage().persistent().get(&key);
    if op.is_some() {
        env.storage()
            .persistent()
            .extend_ttl(&key, BUMP_THRESHOLD, BUMP_AMOUNT);
    }
    op
}

fn put_op(env: &Env, op_id: &BytesN<32>, op: &Operation) {
    let key = DataKey::Operation(op_id.clone());
    env.storage().persistent().set(&key, op);
    env.storage()
        .persistent()
        .extend_ttl(&key, BUMP_THRESHOLD, BUMP_AMOUNT);
}

fn arg<T: TryFromVal<Env, Val>>(env: &Env, args: &Vec<Val>, i: u32) -> T {
    args.get(i)
        .and_then(|v| T::try_from_val(env, &v).ok())
        .unwrap_or_else(|| panic_with_error!(env, Error::InvalidOperation))
}

/// Applies an operation targeting the timelock itself.
fn self_call(env: &Env, func: &Symbol, args: &Vec<Val>) {
    if *func == symbol_short!("set_prop") {
        set_proposer(env, arg(env, args, 0));
    } else if *func == symbol_short!("set_delay") {
        let delay: u64 = arg(env, args, 0);
        env.storage().instance().set(&DataKey::MinDelay, &delay);
        events::min_delay_set(env, delay);
    } else if *func == symbol_short!("set_canc") {
        let canceller: Address = arg(env, args, 0);
        let enabled: bool = arg(env, args, 1);
        let key = DataKey::Canceller(canceller.clone());
        if enabled {
            env.storage().instance().set(&key, &true);
        } else {
            env.storage().instance().remove(&key);
        }
        events::canceller_set(env, canceller, enabled);
    } else if *func == symbol_short!("set_exec") {
        let executor: Option<Address> = arg(env, args, 0);
        match &executor {
            Some(addr) => env.storage().instance().set(&DataKey::Executor, addr),
            None => env.storage().instance().remove(&DataKey::Executor),
        }
        events::executor_set(env, executor);
    } else if *func == symbol_short!("lock_in") {
        env.storage().instance().remove(&DataKey::Guardian);
        events::locked_in(env);
    } else if *func == symbol_short!("migrate") {
        migrations::self_migrate(env, arg(env, args, 0), arg(env, args, 1));
    } else {
        panic_with_error!(env, Error::UnknownSelfCall);
    }
}

#[contractimpl]
impl Timelock {
    pub fn initialize(
        env: Env,
        proposer: Address,
        guardian: Address,
        min_delay: u64,
        grace_period: u64,
    ) {
        extend_instance(&env);
        if env.storage().instance().has(&DataKey::Proposer) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Proposer, &proposer);
        env.storage().instance().set(&DataKey::Guardian, &guardian);
        env.storage().instance().set(&DataKey::MinDelay, &min_delay);
        env.storage()
            .instance()
            .set(&DataKey::GracePeriod, &grace_period);
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    /// Proposer schedules a batch of calls, executable from
    /// `now + delay` until `now + delay + grace_period`.
    pub fn schedule(
        env: Env,
        op_id: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
    ) -> u64 {
        Self::schedule_inner(&env, op_id, None, targets, fns, args, delay)
    }

    /// Like `schedule`, but the operation can only execute once
    /// `predecessor` has. `predecessor` must already be scheduled (or
    /// executed), which is what keeps dependencies acyclic.
    pub fn schedule_after(
        env: Env,
        op_id: BytesN<32>,
        predecessor: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
    ) -> u64 {
        Self::schedule_inner(&env, op_id, Some(predecessor), targets, fns, args, delay)
    }

    /// Executes a ready, unexpired operation whose predecessor (if any)
    /// already ran. Open to anyone unless an executor is configured.
    pub fn execute(env: Env, op_id: BytesN<32>) {
        extend_instance(&env);
        migrations::require_current(&env);
        if let Some(executor) = Self::get_executor(env.clone()) {
            executor.require_auth();
        }
        let mut op = get_op(&env, &op_id)
            .unwrap_or_else(|| panic_with_error!(&env, Error::OperationNotFound));
        if op.done {
            panic_with_error!(&env, Error::AlreadyDone);
        }
        let now = env.ledger().timestamp();
        if now < op.ready_at {
            panic_with_error!(&env, Error::NotReady);
        }
        if now
            > op.ready_at
                .saturating_add(Self::get_grace_period(env.clone()))
        {
            panic_with_error!(&env, Error::Expired);
        }
        for pred in op.predecessors.iter() {
            if !Self::is_done(env.clone(), pred) {
                panic_with_error!(&env, Error::PredecessorNotDone);
            }
        }
        op.done = true;
        put_op(&env, &op_id, &op);

        let this = env.current_contract_address();
        for i in 0..op.targets.len() {
            let target = op.targets.get_unchecked(i);
            let func = op.fns.get_unchecked(i);
            let args = op.args.get_unchecked(i);
            if target == this {
                self_call(&env, &func, &args);
            } else {
                env.invoke_contract::<Val>(&target, &func, args);
            }
        }
        events::executed(&env, op_id);
    }

    /// A canceller (proposer, guardian before lock-in, or a granted
    /// canceller) removes a pending operation.
    pub fn cancel(env: Env, caller: Address, op_id: BytesN<32>) {
        extend_instance(&env);
        caller.require_auth();
        if !Self::is_canceller(env.clone(), caller.clone()) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        let op = get_op(&env, &op_id)
            .unwrap_or_else(|| panic_with_error!(&env, Error::OperationNotFound));
        if op.done {
            panic_with_error!(&env, Error::AlreadyDone);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::Operation(op_id.clone()));
        events::canceled(&env, op_id, caller);
    }

    /// Guardian-only rollback: reassigns the proposer immediately, e.g.
    /// from the governor back to the multisig. Unavailable after lock-in.
    pub fn guardian_set_proposer(env: Env, new_proposer: Address) {
        extend_instance(&env);
        let guardian = Self::get_guardian(env.clone())
            .unwrap_or_else(|| panic_with_error!(&env, Error::LockedIn));
        guardian.require_auth();
        set_proposer(&env, new_proposer);
    }

    // ── Views ────────────────────────────────────────────────────────────────

    pub fn get_proposer(env: Env) -> Address {
        extend_instance(&env);
        proposer(&env)
    }

    /// None once locked in.
    pub fn get_guardian(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Guardian)
    }

    pub fn is_locked_in(env: Env) -> bool {
        !env.storage().instance().has(&DataKey::Guardian)
    }

    /// True for the proposer, the guardian (before lock-in) and any
    /// address granted with `set_canc`.
    pub fn is_canceller(env: Env, who: Address) -> bool {
        who == proposer(&env)
            || Self::get_guardian(env.clone()) == Some(who.clone())
            || env.storage().instance().has(&DataKey::Canceller(who))
    }

    /// None means execution is open to anyone.
    pub fn get_executor(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Executor)
    }

    pub fn get_min_delay(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MinDelay)
            .unwrap_or(0)
    }

    pub fn get_grace_period(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::GracePeriod)
            .unwrap_or(0)
    }

    /// True once the operation has been executed.
    pub fn is_done(env: Env, op_id: BytesN<32>) -> bool {
        get_op(&env, &op_id).is_some_and(|op| op.done)
    }

    /// Scheduled, not yet executed, and not cancelled.
    pub fn is_pending(env: Env, op_id: BytesN<32>) -> bool {
        get_op(&env, &op_id).is_some_and(|op| !op.done)
    }

    /// Pending, its delay has elapsed and its grace period hasn't, and
    /// its predecessor (if any) is done — i.e. `execute` would run it.
    pub fn is_ready(env: Env, op_id: BytesN<32>) -> bool {
        let Some(op) = get_op(&env, &op_id) else {
            return false;
        };
        let now = env.ledger().timestamp();
        !op.done
            && now >= op.ready_at
            && now
                <= op
                    .ready_at
                    .saturating_add(Self::get_grace_period(env.clone()))
            && op
                .predecessors
                .iter()
                .all(|pred| Self::is_done(env.clone(), pred))
    }

    pub fn get_operation(env: Env, op_id: BytesN<32>) -> Option<Operation> {
        get_op(&env, &op_id)
    }
}

impl Timelock {
    #[allow(clippy::too_many_arguments)]
    fn schedule_inner(
        env: &Env,
        op_id: BytesN<32>,
        predecessor: Option<BytesN<32>>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
    ) -> u64 {
        extend_instance(env);
        migrations::require_current(env);
        proposer(env).require_auth();
        if targets.is_empty() || targets.len() != fns.len() || targets.len() != args.len() {
            panic_with_error!(env, Error::InvalidOperation);
        }
        if delay < Self::get_min_delay(env.clone()) {
            panic_with_error!(env, Error::DelayTooShort);
        }
        if get_op(env, &op_id).is_some() {
            panic_with_error!(env, Error::OperationExists);
        }
        let mut predecessors = Vec::new(env);
        if let Some(pred) = predecessor {
            if pred == op_id || get_op(env, &pred).is_none() {
                panic_with_error!(env, Error::PredecessorNotFound);
            }
            predecessors.push_back(pred);
        }
        let ready_at = env
            .ledger()
            .timestamp()
            .checked_add(delay)
            .unwrap_or_else(|| panic_with_error!(env, Error::InvalidOperation));
        put_op(
            env,
            &op_id,
            &Operation {
                targets,
                fns,
                args,
                ready_at,
                done: false,
                predecessors: predecessors.clone(),
            },
        );
        events::scheduled(env, op_id, ready_at, predecessors);
        ready_at
    }
}
