#![no_std]

//! Zenith Timelock — delays every privileged call by at least `min_delay`
//! so users can react (or a guardian can cancel) before it lands.
//!
//! A single proposer schedules batches of contract calls; once ready,
//! anyone may execute them (an "open executor", as in OpenZeppelin's
//! TimelockController). Execution being permissionless is also what lets a
//! governor proposal target the governor itself: the governor never sits
//! in the call stack, so Soroban's no-re-entry rule is not tripped.
//! Contracts that hand their admin to this timelock accept those calls
//! because the timelock is the direct invoker, so `admin.require_auth()`
//! passes without any signature.
//!
//! Soroban forbids a contract re-entering itself, so the timelock's own
//! configuration (`set_proposer`, `set_min_delay`, `lock_in`) is changed by
//! scheduling an operation whose target is the timelock itself; `execute`
//! recognises those and applies them in place instead of cross-calling.
//!
//! Until `lock_in`, the guardian can cancel any pending operation and
//! reassign the proposer (the rollback path of the governance handover,
//! see docs/governance-handover.md). After `lock_in` the guardian is gone
//! for good.
//!
//! An operation may name `predecessors` (`schedule_after`) that must all
//! be executed first. Each predecessor has to be scheduled already, so
//! self-edges and cycles are rejected at schedule time, and a cancelled
//! operation leaves a tombstone: its id can never be scheduled again, so
//! anything depending on it is permanently unexecutable. Execution is open
//! unless an executor is set through the `set_exec` self-call.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, symbol_short, Address, BytesN, Env, Symbol,
    TryFromVal, Val, Vec,
};

#[cfg(test)]
mod proptest;
#[cfg(test)]
mod test;

mod error;
mod events;
mod migrations;
mod types;

use error::Error;
use types::DataKey;
pub use types::Operation;

#[contract]
pub struct Timelock;

fn proposer(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Proposer).unwrap()
}

fn set_proposer(env: &Env, new: Address) {
    events::proposer_set(env, proposer(env), new.clone());
    env.storage().instance().set(&DataKey::Proposer, &new);
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
    } else if *func == symbol_short!("lock_in") {
        env.storage().instance().remove(&DataKey::Guardian);
        events::locked_in(env);
    } else if *func == symbol_short!("set_exec") {
        let executor: Address = arg(env, args, 0);
        env.storage().instance().set(&DataKey::Executor, &executor);
        events::executor_set(env, Some(executor));
    } else if *func == symbol_short!("open_exec") {
        env.storage().instance().remove(&DataKey::Executor);
        events::executor_set(env, None);
    } else if *func == symbol_short!("migrate") {
        migrations::self_migrate(env, arg(env, args, 0), arg(env, args, 1));
    } else {
        panic_with_error!(env, Error::UnknownSelfCall);
    }
}

fn schedule_op(
    env: &Env,
    op_id: BytesN<32>,
    targets: Vec<Address>,
    fns: Vec<Symbol>,
    args: Vec<Vec<Val>>,
    delay: u64,
    predecessors: Vec<BytesN<32>>,
) -> u64 {
    migrations::require_current(env);
    proposer(env).require_auth();
    if targets.is_empty() || targets.len() != fns.len() || targets.len() != args.len() {
        panic_with_error!(env, Error::InvalidOperation);
    }
    if delay < Timelock::get_min_delay(env.clone()) {
        panic_with_error!(env, Error::DelayTooShort);
    }
    let key = DataKey::Operation(op_id.clone());
    if env.storage().persistent().has(&key) || Timelock::is_cancelled(env.clone(), op_id.clone()) {
        panic_with_error!(env, Error::OperationExists);
    }
    for pred in predecessors.iter() {
        if pred == op_id || Timelock::get_operation(env.clone(), pred).is_none() {
            panic_with_error!(env, Error::InvalidPredecessor);
        }
    }
    let ready_at = env.ledger().timestamp() + delay;
    env.storage().persistent().set(
        &key,
        &Operation {
            targets,
            fns,
            args,
            ready_at,
            done: false,
            predecessors,
        },
    );
    events::scheduled(env, op_id, ready_at);
    ready_at
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
    /// `now + delay` until `now + delay + grace_period`. Calls targeting
    /// the timelock itself may use: `set_prop(Address)`,
    /// `set_delay(u64)`, `lock_in()`, `set_exec(Address)`, `open_exec()`,
    /// `migrate(u32, u32)`.
    pub fn schedule(
        env: Env,
        op_id: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
    ) -> u64 {
        schedule_op(&env, op_id, targets, fns, args, delay, Vec::new(&env))
    }

    /// `schedule`, but the operation can only execute once every one of
    /// `predecessors` has. Each must already be scheduled (pending or
    /// done) and differ from `op_id`, so dependency graphs stay acyclic.
    pub fn schedule_after(
        env: Env,
        op_id: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
        predecessors: Vec<BytesN<32>>,
    ) -> u64 {
        schedule_op(&env, op_id, targets, fns, args, delay, predecessors)
    }

    /// Executes a ready, unexpired operation whose predecessors are all
    /// done. Open to anyone unless an executor is set.
    pub fn execute(env: Env, op_id: BytesN<32>) {
        if let Some(executor) = Self::get_executor(env.clone()) {
            executor.require_auth();
        }
        let key = DataKey::Operation(op_id.clone());
        let mut op: Operation = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::OperationNotFound));
        if op.done {
            panic_with_error!(&env, Error::AlreadyDone);
        }
        let now = env.ledger().timestamp();
        if now < op.ready_at {
            panic_with_error!(&env, Error::NotReady);
        }
        if now > op.ready_at + Self::get_grace_period(env.clone()) {
            panic_with_error!(&env, Error::Expired);
        }
        for pred in op.predecessors.iter() {
            if !Self::is_done(env.clone(), pred) {
                panic_with_error!(&env, Error::PredecessorNotDone);
            }
        }
        op.done = true;
        env.storage().persistent().set(&key, &op);

        let this = env.current_contract_address();
        for i in 0..op.targets.len() {
            let target = op.targets.get(i).unwrap();
            let func = op.fns.get(i).unwrap();
            let args = op.args.get(i).unwrap();
            if target == this {
                self_call(&env, &func, &args);
            } else {
                env.invoke_contract::<Val>(&target, &func, args);
            }
        }
        events::executed(&env, op_id);
    }

    /// Proposer or (before lock-in) guardian cancels a pending operation.
    pub fn cancel(env: Env, caller: Address, op_id: BytesN<32>) {
        caller.require_auth();
        if caller != proposer(&env) && Some(caller.clone()) != Self::get_guardian(env.clone()) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        let key = DataKey::Operation(op_id.clone());
        let op: Operation = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::OperationNotFound));
        if op.done {
            panic_with_error!(&env, Error::AlreadyDone);
        }
        env.storage().persistent().remove(&key);
        env.storage()
            .persistent()
            .set(&DataKey::Cancelled(op_id.clone()), &());
        events::canceled(&env, op_id, caller);
    }

    /// Guardian-only rollback: reassigns the proposer immediately, e.g.
    /// from the governor back to the multisig. Unavailable after lock-in.
    pub fn guardian_set_proposer(env: Env, new_proposer: Address) {
        let guardian = Self::get_guardian(env.clone())
            .unwrap_or_else(|| panic_with_error!(&env, Error::LockedIn));
        guardian.require_auth();
        set_proposer(&env, new_proposer);
    }

    // ── Views ────────────────────────────────────────────────────────────────

    pub fn get_proposer(env: Env) -> Address {
        proposer(&env)
    }

    /// None once locked in.
    pub fn get_guardian(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Guardian)
    }

    pub fn is_locked_in(env: Env) -> bool {
        !env.storage().instance().has(&DataKey::Guardian)
    }

    pub fn get_min_delay(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::MinDelay).unwrap()
    }

    pub fn get_grace_period(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::GracePeriod).unwrap()
    }

    /// True once the operation has been executed.
    pub fn is_done(env: Env, op_id: BytesN<32>) -> bool {
        Self::get_operation(env, op_id).is_some_and(|op| op.done)
    }

    pub fn get_operation(env: Env, op_id: BytesN<32>) -> Option<Operation> {
        env.storage().persistent().get(&DataKey::Operation(op_id))
    }

    /// True once the operation has been cancelled; its id stays retired.
    pub fn is_cancelled(env: Env, op_id: BytesN<32>) -> bool {
        env.storage().persistent().has(&DataKey::Cancelled(op_id))
    }

    /// None while execution is open to anyone.
    pub fn get_executor(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Executor)
    }
}
