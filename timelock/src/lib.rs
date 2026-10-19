#![no_std]

//! Zenith Timelock — a delayed-execution admin.
//!
//! The timelock is meant to hold the `admin` role on every other Zenith
//! contract, so any parameter or code change is visible on-chain for at least
//! `min_delay` seconds before it can run. Users get an exit window; a
//! canceller gets a veto. Modelled on OpenZeppelin's TimelockController.
//!
//! Three roles, and no admin:
//!
//!   Proposer   may schedule an operation.
//!   Executor   may execute a ready operation (anyone may, if the executor
//!              set is empty — see `is_open_executor`).
//!   Canceller  may cancel a pending operation.
//!
//! There is deliberately no fourth role and no owner: the timelock's own
//! configuration is changed the same way everything else is, by scheduling an
//! operation whose target is the timelock itself. Soroban forbids a contract
//! re-entering itself, so `execute` recognises those calls and applies them
//! in place instead of cross-calling.
//!
//! An operation is a batch of calls plus a predecessor and a salt. Its id is
//! `sha256(xdr((targets, fns, args, predecessor, salt)))` — it commits to
//! exactly what will run, and the salt is what makes two otherwise identical
//! batches distinct. `execute` recomputes the id from the batch it is handed,
//! so it can only ever run the batch that was scheduled.
//!
//! A predecessor orders operations against each other: an operation whose
//! predecessor has not executed cannot execute. Cancelling a predecessor
//! therefore leaves its dependants permanently unexecutable, which is the
//! point: the batch that was going to run second never runs at all.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, xdr::ToXdr, Address, BytesN, Env, Symbol,
    TryFromVal, Val, Vec,
};

#[cfg(test)]
mod test;

mod error;
mod events;
mod migrations;
mod types;

use error::Error;
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::DataKey;
pub use types::{Operation, OperationSpec, Role};

/// The shortest delay the timelock will ever accept, in seconds.
///
/// A delay of zero is not a delay: the operation would be executable in the
/// same ledger it was scheduled in, and the exit window this contract exists
/// to create would not exist at all. One minute is the floor — long enough
/// that the operation is a separate, announced event, short enough that it
/// does not stand in the way of a test or a testnet rehearsal.
pub const MIN_MIN_DELAY: u64 = 60;

#[contract]
pub struct Timelock;

// ── internals ────────────────────────────────────────────────────────────────

fn min_delay(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::MinDelay)
        .unwrap_or(MIN_MIN_DELAY)
}

fn role_key(role: Role, account: &Address) -> DataKey {
    match role {
        Role::Proposer => DataKey::Proposer(account.clone()),
        Role::Executor => DataKey::Executor(account.clone()),
        Role::Canceller => DataKey::Canceller(account.clone()),
    }
}

fn holds_role(env: &Env, role: Role, account: &Address) -> bool {
    env.storage().instance().has(&role_key(role, account))
}

/// Panics unless `account` holds `role`.
fn require_role(env: &Env, role: Role, account: &Address) {
    if !holds_role(env, role, account) {
        panic_with_error!(env, Error::Unauthorized);
    }
}

fn set_role(env: &Env, role: Role, account: &Address, granted: bool) {
    let key = role_key(role, account);
    if granted {
        env.storage().instance().set(&key, &true);
    } else {
        env.storage().instance().remove(&key);
    }
    events::role_changed(env, role, account.clone(), granted);
}

fn load(env: &Env, id: &BytesN<32>) -> Option<Operation> {
    env.storage().persistent().get(&DataKey::Operation(id.clone()))
}

/// The `i`th argument of a self-call, decoded.
fn arg<T: TryFromVal<Env, Val>>(env: &Env, args: &Vec<Val>, i: u32) -> T {
    args.get(i)
        .and_then(|v| T::try_from_val(env, &v).ok())
        .unwrap_or_else(|| panic_with_error!(env, Error::UnknownSelfCall))
}

/// Applies a call whose target is the timelock itself.
///
/// These are the only way to change the timelock's configuration, and they
/// are only reachable through a delayed operation — so widening the delay or
/// granting yourself a role takes as long to take effect as any other
/// operation.
fn self_call(env: &Env, func: &Symbol, args: &Vec<Val>) {
    if *func == Symbol::new(env, "update_delay") {
        let delay: u64 = arg(env, args, 0);
        if delay < MIN_MIN_DELAY {
            panic_with_error!(env, Error::InsufficientDelay);
        }
        let old = min_delay(env);
        env.storage().instance().set(&DataKey::MinDelay, &delay);
        events::min_delay_changed(env, old, delay);
    } else if *func == Symbol::new(env, "grant_role") {
        let role: Role = arg(env, args, 0);
        let account: Address = arg(env, args, 1);
        set_role(env, role, &account, true);
    } else if *func == Symbol::new(env, "revoke_role") {
        let role: Role = arg(env, args, 0);
        let account: Address = arg(env, args, 1);
        set_role(env, role, &account, false);
    } else if *func == Symbol::new(env, "set_open_executor") {
        let open: bool = arg(env, args, 0);
        env.storage().instance().set(&DataKey::OpenExecutor, &open);
    } else if *func == Symbol::new(env, "migrate") {
        let from: u32 = arg(env, args, 0);
        let to: u32 = arg(env, args, 1);
        migrations::self_migrate(env, from, to);
    } else {
        panic_with_error!(env, Error::UnknownSelfCall);
    }
}

#[contractimpl]
impl Timelock {
    /// One-time setup. There is no admin afterwards: the three role sets are
    /// the whole configuration, and they can only be changed by a delayed
    /// operation.
    ///
    /// An empty `executors` list means anyone may execute a ready operation.
    ///
    /// `grace_period` is how long a ready operation stays executable. Past
    /// it the operation is dead and has to be scheduled again: without that
    /// bound an operation announced months ago could still land tomorrow, and
    /// the exit window it was reviewed for would have long since closed.
    pub fn initialize(
        env: Env,
        min_delay: u64,
        grace_period: u64,
        proposers: Vec<Address>,
        executors: Vec<Address>,
        cancellers: Vec<Address>,
    ) {
        if env.storage().instance().has(&DataKey::Initialized) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        if min_delay < MIN_MIN_DELAY {
            panic_with_error!(&env, Error::InsufficientDelay);
        }
        env.storage().instance().set(&DataKey::MinDelay, &min_delay);
        env.storage().instance().set(&DataKey::GracePeriod, &grace_period);
        for p in proposers.iter() {
            env.storage().instance().set(&DataKey::Proposer(p.clone()), &true);
        }
        for e in executors.iter() {
            env.storage().instance().set(&DataKey::Executor(e.clone()), &true);
        }
        for c in cancellers.iter() {
            env.storage()
                .instance()
                .set(&DataKey::Canceller(c.clone()), &true);
        }
        env.storage()
            .instance()
            .set(&DataKey::OpenExecutor, &executors.is_empty());
        env.storage().instance().set(&DataKey::Initialized, &true);
        zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION);
    }

    /// Schedules a batch of calls for execution no earlier than
    /// `now + delay`. Proposer-only, and `delay` may not be shorter than
    /// `min_delay`.
    ///
    /// Returns the operation id, which is derived from the batch itself: the
    /// same batch with the same predecessor and the same salt is the same
    /// operation, and scheduling it twice is refused.
    pub fn schedule(
        env: Env,
        proposer: Address,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
        delay: u64,
    ) -> BytesN<32> {
        migrations::require_current(&env);
        proposer.require_auth();
        require_role(&env, Role::Proposer, &proposer);

        let n = targets.len();
        if n == 0 || n != fns.len() || n != args.len() {
            panic_with_error!(&env, Error::EmptyOperation);
        }
        if delay < Self::get_min_delay(env.clone()) {
            panic_with_error!(&env, Error::InsufficientDelay);
        }

        let id = Self::hash_operation(
            env.clone(),
            targets.clone(),
            fns.clone(),
            args.clone(),
            predecessor.clone(),
            salt.clone(),
        );
        let key = DataKey::Operation(id.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, Error::AlreadyScheduled);
        }

        let ready_at = env.ledger().timestamp() + delay;
        env.storage().persistent().set(
            &key,
            &Operation {
                targets,
                fns,
                args,
                predecessor,
                salt,
                ready_at,
                done: false,
            },
        );
        events::scheduled(&env, id.clone(), proposer, ready_at);
        id
    }

    /// Executes a ready operation. The batch, the predecessor and the salt
    /// are passed again because the id is derived from them: only the batch
    /// that was scheduled can be executed.
    ///
    /// Every call runs, in order. A call that reverts reverts the whole
    /// operation — there is no partial application.
    pub fn execute(
        env: Env,
        executor: Address,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
    ) {
        migrations::require_current(&env);
        executor.require_auth();
        if !Self::is_open_executor(env.clone()) {
            require_role(&env, Role::Executor, &executor);
        }

        let id = Self::hash_operation(
            env.clone(),
            targets,
            fns,
            args,
            predecessor.clone(),
            salt.clone(),
        );
        let key = DataKey::Operation(id.clone());
        let mut op: Operation = load(&env, &id).unwrap_or_else(|| {
            panic_with_error!(&env, Error::NotPending)
        });
        if op.done {
            panic_with_error!(&env, Error::NotPending);
        }

        let now = env.ledger().timestamp();
        if now < op.ready_at {
            panic_with_error!(&env, Error::NotReady);
        }
        if now > op.ready_at + Self::get_grace_period(env.clone()) {
            panic_with_error!(&env, Error::Expired);
        }

        // The predecessor must have executed. A cancelled predecessor is
        // gone from storage, so it reads as "not done" here: the dependant
        // stays unexecutable rather than running out of order.
        if let Some(pred) = op.predecessor.clone() {
            let done = load(&env, &pred).is_some_and(|p| p.done);
            if !done {
                panic_with_error!(&env, Error::PredecessorNotDone);
            }
        }

        op.done = true;
        env.storage().persistent().set(&key, &op);

        let this = env.current_contract_address();
        let n = op.targets.len();
        for i in 0..n {
            let target = op.targets.get(i).unwrap();
            let func = op.fns.get(i).unwrap();
            let call_args = op.args.get(i).unwrap();
            if target == this {
                self_call(&env, &func, &call_args);
            } else {
                env.invoke_contract::<Val>(&target, &func, call_args);
            }
        }
        events::executed(&env, id);
    }

    /// Cancels a pending operation. Canceller-only.
    ///
    /// The operation's record is removed, so it can never be executed and
    /// anything waiting on it as a predecessor is stuck for good.
    pub fn cancel(env: Env, canceller: Address, id: BytesN<32>) {
        migrations::require_current(&env);
        canceller.require_auth();
        require_role(&env, Role::Canceller, &canceller);

        let key = DataKey::Operation(id.clone());
        let op: Operation =
            load(&env, &id).unwrap_or_else(|| panic_with_error!(&env, Error::NotPending));
        if op.done {
            panic_with_error!(&env, Error::NotPending);
        }
        env.storage().persistent().remove(&key);
        events::cancelled(&env, id, canceller);
    }

    // ── Views ────────────────────────────────────────────────────────────────

    /// The shortest delay an operation may be scheduled with, in seconds.
    pub fn get_min_delay(env: Env) -> u64 {
        min_delay(&env)
    }

    /// How long a ready operation stays executable, in seconds.
    pub fn get_grace_period(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::GracePeriod)
            .unwrap_or(0)
    }

    /// True when anyone may execute a ready operation.
    pub fn is_open_executor(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::OpenExecutor)
            .unwrap_or(false)
    }

    /// True when `account` holds `role`.
    pub fn has_role(env: Env, role: Role, account: Address) -> bool {
        holds_role(&env, role, &account)
    }

    /// `0` for an operation that does not exist, `1` for one that has
    /// executed, and otherwise the timestamp it becomes ready at.
    pub fn get_timestamp(env: Env, id: BytesN<32>) -> u64 {
        match load(&env, &id) {
            None => 0,
            Some(op) if op.done => 1,
            Some(op) => op.ready_at,
        }
    }

    /// True for any operation that exists, pending or done.
    pub fn is_operation(env: Env, id: BytesN<32>) -> bool {
        env.storage().persistent().has(&DataKey::Operation(id))
    }

    /// True for an operation that exists and has not executed.
    pub fn is_operation_pending(env: Env, id: BytesN<32>) -> bool {
        load(&env, &id).is_some_and(|op| !op.done)
    }

    /// True for an operation whose delay has elapsed.
    pub fn is_operation_ready(env: Env, id: BytesN<32>) -> bool {
        load(&env, &id).is_some_and(|op| !op.done && env.ledger().timestamp() >= op.ready_at)
    }

    /// True for an operation that has executed.
    pub fn is_operation_done(env: Env, id: BytesN<32>) -> bool {
        load(&env, &id).is_some_and(|op| op.done)
    }

    /// The stored operation, if there is one.
    pub fn get_operation(env: Env, id: BytesN<32>) -> Option<Operation> {
        load(&env, &id)
    }

    /// The id of the operation a batch, a predecessor and a salt describe.
    ///
    /// The same formula `schedule` and `execute` use, exposed so an
    /// integrator can compute an id off-chain or in another contract.
    pub fn hash_operation(
        env: Env,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
    ) -> BytesN<32> {
        let spec = OperationSpec {
            targets,
            fns,
            args,
            predecessor,
            salt,
        };
        env.crypto().sha256(&spec.clone().to_xdr(&env)).into()
    }
}
