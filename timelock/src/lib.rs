#![no_std]

//! Zenith Timelock — a delayed-execution admin.
//!
//! Every privileged call waits. A batch of calls is scheduled by a proposer,
//! becomes executable only after `min_delay` has elapsed, and can be
//! aborted by a canceller until it runs. That is the whole point: the
//! timelock is meant to hold the `admin` role on the other contracts, so
//! users always get an exit window before a destructive action lands.
//!
//! ## Roles
//!
//! Each role is a *set* of addresses, not a single holder:
//!
//! | Role        | May                                                     |
//! |-------------|---------------------------------------------------------|
//! | `Proposer`  | schedule operations                                     |
//! | `Executor`  | execute a ready operation                               |
//! | `Canceller` | cancel a pending operation                              |
//!
//! An **empty** executor set means anyone may execute a ready operation
//! ("open executor"), the same escape hatch OpenZeppelin's
//! TimelockController has. The executor set can also be turned into an open
//! one at runtime with the `set_open_executor` self-call.
//!
//! There is no admin: the timelock's own configuration is changed only
//! through a delayed operation, like every other privileged call.
//!
//! Contracts that hand their admin to this timelock accept its calls
//! because the timelock is the direct invoker, so `admin.require_auth()`
//! passes without any signature. Those contracts stay agnostic about
//! whether their admin is a key or a contract — the same `Auth::Admin`
//! shape `zenith-common`'s `require_admin_or_multisig` already speaks.
//!
//! ## Operation ids
//!
//! An operation is named by a 32-byte id: the SHA-256 of the XDR of
//! `(calls, predecessor, salt)`. The id therefore commits to the whole
//! payload, and `hash_operation` lets anyone derive it off-chain before
//! scheduling anything. The `salt` is what makes two otherwise identical
//! batches two different operations.
//!
//! ## Ordering
//!
//! An operation may name a **predecessor**: it can only run once that one
//! has. A predecessor that is cancelled can never run, so neither can its
//! dependent — a cancelled predecessor never leaves a dependent executable.
//!
//! ## Changing the timelock itself
//!
//! Soroban forbids a contract re-entering itself, so the timelock's own
//! configuration is changed by scheduling a batch whose target is the
//! timelock itself. `execute` recognises those and applies them in place
//! instead of cross-calling: `update_delay(u64)`, `grant_role(Role,
//! Address)`, `revoke_role(Role, Address)`, `set_open_executor(bool)` and
//! `migrate(u32, u32)`.
//!
//! Every delay is measured on the ledger **timestamp** — never on ledger
//! counts.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, vec, xdr::ToXdr, Address, BytesN, Env, IntoVal,
    Symbol, TryFromVal, Val, Vec,
};

#[cfg(test)]
mod test;

pub mod error;
mod events;
mod migrations;
pub mod types;

pub use error::Error;
use types::{is_no_predecessor, DataKey, Operation};
pub use types::{Call, Call as TimelockCall, Role, Role as TimelockRole};

#[contract]
pub struct Timelock;

// ── storage helpers ──────────────────────────────────────────────────────────

fn min_delay(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::MinDelay)
        .unwrap_or(0)
}

fn open_executor(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::OpenExecutor)
        .unwrap_or(false)
}

fn has_role(env: &Env, role: Role, account: &Address) -> bool {
    env.storage()
        .instance()
        .has(&DataKey::Role(role, account.clone()))
}

fn grant_role(env: &Env, role: Role, account: &Address) {
    env.storage()
        .instance()
        .set(&DataKey::Role(role, account.clone()), &true);
    events::role_changed(env, role, account.clone(), true);
}

fn revoke_role(env: &Env, role: Role, account: &Address) {
    env.storage()
        .instance()
        .remove(&DataKey::Role(role, account.clone()));
    events::role_changed(env, role, account.clone(), false);
}

/// The role is checked before the signature, so "who may do this" is
/// answered by the contract and not only by the signature.
fn require_role(env: &Env, role: Role, account: &Address) {
    if !has_role(env, role, account) {
        panic_with_error!(env, Error::Unauthorized);
    }
}

fn load_op(env: &Env, id: &BytesN<32>) -> Option<Operation> {
    env.storage()
        .persistent()
        .get(&DataKey::Operation(id.clone()))
}

fn is_done(env: &Env, id: &BytesN<32>) -> bool {
    load_op(env, id).is_some_and(|op| op.done)
}

fn arg<T: TryFromVal<Env, Val>>(env: &Env, args: &Vec<Val>, i: u32) -> T {
    args.get(i)
        .and_then(|v| T::try_from_val(env, &v).ok())
        .unwrap_or_else(|| panic_with_error!(env, Error::EmptyOperation))
}

/// The id of the operation made of `calls`, `predecessor` and `salt`: the
/// SHA-256 of their XDR, so the id commits to the whole payload and anyone
/// can derive it before scheduling anything.
pub fn hash_operation(
    env: &Env,
    calls: &Vec<Call>,
    predecessor: &Option<BytesN<32>>,
    salt: &BytesN<32>,
) -> BytesN<32> {
    let input: Vec<Val> = vec![
        env,
        calls.clone().into_val(env),
        predecessor.clone().into_val(env),
        salt.clone().into_val(env),
    ];
    env.crypto().sha256(&input.to_xdr(env)).into()
}

/// Applies a batch whose target is the timelock itself.
fn self_call(env: &Env, func: &Symbol, args: &Vec<Val>) {
    if *func == Symbol::new(env, "update_delay") {
        let old = min_delay(env);
        let new: u64 = arg(env, args, 0);
        env.storage().instance().set(&DataKey::MinDelay, &new);
        events::min_delay_changed(env, old, new);
    } else if *func == Symbol::new(env, "grant_role") {
        let role: Role = arg(env, args, 0);
        let account: Address = arg(env, args, 1);
        grant_role(env, role, &account);
    } else if *func == Symbol::new(env, "revoke_role") {
        let role: Role = arg(env, args, 0);
        let account: Address = arg(env, args, 1);
        revoke_role(env, role, &account);
    } else if *func == Symbol::new(env, "set_open_executor") {
        let open: bool = arg(env, args, 0);
        env.storage().instance().set(&DataKey::OpenExecutor, &open);
        events::open_executor_changed(env, open);
    } else if *func == Symbol::new(env, "migrate") {
        if let Err(e) = migrations::self_migrate(env, arg(env, args, 0), arg(env, args, 1)) {
            panic_with_error!(env, e);
        }
    } else {
        panic_with_error!(env, Error::UnknownSelfCall);
    }
}

#[contractimpl]
impl Timelock {
    /// One-time setup. There is no admin afterwards: the timelock's own
    /// configuration changes only through a delayed operation.
    ///
    /// An empty `executors` list means anyone may execute a ready
    /// operation.
    pub fn initialize(
        env: Env,
        min_delay: u64,
        proposers: Vec<Address>,
        executors: Vec<Address>,
        cancellers: Vec<Address>,
    ) {
        if env.storage().instance().has(&DataKey::MinDelay) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::MinDelay, &min_delay);
        env.storage()
            .instance()
            .set(&DataKey::OpenExecutor, &executors.is_empty());
        for proposer in proposers.iter() {
            grant_role(&env, Role::Proposer, &proposer);
        }
        for executor in executors.iter() {
            grant_role(&env, Role::Executor, &executor);
        }
        for canceller in cancellers.iter() {
            grant_role(&env, Role::Canceller, &canceller);
        }
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    /// Schedules a batch of calls. Proposer only.
    ///
    /// The batch becomes executable at `now + delay` and stays executable
    /// until it is run or cancelled. The id is the SHA-256 of the XDR of
    /// `(calls, predecessor, salt)`, so it commits to the whole payload;
    /// `hash_operation` derives the same value.
    ///
    /// Returns the id of the scheduled operation.
    pub fn schedule(
        env: Env,
        proposer: Address,
        calls: Vec<Call>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
        delay: u64,
    ) -> BytesN<32> {
        migrations::require_current(&env);
        require_role(&env, Role::Proposer, &proposer);
        proposer.require_auth();
        if calls.is_empty() {
            panic_with_error!(&env, Error::EmptyOperation);
        }
        if delay < min_delay(&env) {
            panic_with_error!(&env, Error::InsufficientDelay);
        }
        let id = hash_operation(&env, &calls, &predecessor, &salt);
        let key = DataKey::Operation(id.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, Error::AlreadyScheduled);
        }
        let ready_at = env
            .ledger()
            .timestamp()
            .checked_add(delay)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InsufficientDelay));
        env.storage().persistent().set(
            &key,
            &Operation {
                calls: calls.clone(),
                predecessor: predecessor
                    .clone()
                    .unwrap_or_else(|| BytesN::from_array(&env, &[0u8; 32])),
                salt: salt.clone(),
                ready_at,
                done: false,
            },
        );
        events::scheduled(&env, id.clone(), ready_at, predecessor);
        id
    }

    /// Runs a ready operation. Executor only, unless the executor set is
    /// empty.
    ///
    /// The batch is identified by its content: the caller repeats the
    /// `calls`, `predecessor` and `salt` it was scheduled with, and the id
    /// they hash to is the operation that runs. The delay boundary is
    /// inclusive: an operation may be executed at exactly its ready-at, and
    /// not before.
    pub fn execute(
        env: Env,
        executor: Address,
        calls: Vec<Call>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
    ) {
        migrations::require_current(&env);
        if !open_executor(&env) {
            require_role(&env, Role::Executor, &executor);
            executor.require_auth();
        }
        let id = hash_operation(&env, &calls, &predecessor, &salt);
        let key = DataKey::Operation(id.clone());
        let mut op: Operation = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotPending));
        if op.done {
            panic_with_error!(&env, Error::NotPending);
        }
        let now = env.ledger().timestamp();
        if now < op.ready_at {
            panic_with_error!(&env, Error::NotReady);
        }
        if !is_no_predecessor(&op.predecessor) && !is_done(&env, &op.predecessor) {
            panic_with_error!(&env, Error::PredecessorNotDone);
        }
        op.done = true;
        env.storage().persistent().set(&key, &op);

        let this = env.current_contract_address();
        for call in op.calls.iter() {
            if call.target == this {
                self_call(&env, &call.function, &call.args);
            } else {
                env.invoke_contract::<Val>(&call.target, &call.function, call.args.clone());
            }
        }
        events::executed(&env, id);
    }

    /// Cancels a pending operation. Canceller only.
    ///
    /// A predecessor that is cancelled can never run, so neither can its
    /// dependent: cancelling a predecessor never leaves a dependent
    /// executable.
    pub fn cancel(env: Env, canceller: Address, id: BytesN<32>) {
        migrations::require_current(&env);
        require_role(&env, Role::Canceller, &canceller);
        canceller.require_auth();
        let key = DataKey::Operation(id.clone());
        let op: Operation = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotPending));
        if op.done {
            panic_with_error!(&env, Error::NotPending);
        }
        env.storage().persistent().remove(&key);
        events::cancelled(&env, id);
    }

    // ── Views ────────────────────────────────────────────────────────────────

    /// The operation's status, packed into one number: `0` when there is no
    /// such operation, `1` when it has run, and its ready-at timestamp
    /// otherwise.
    pub fn get_timestamp(env: Env, id: BytesN<32>) -> u64 {
        match load_op(&env, &id) {
            Some(op) => {
                if op.done {
                    1
                } else {
                    op.ready_at
                }
            }
            None => 0,
        }
    }

    /// Whether an operation with this id exists (pending or done).
    pub fn is_operation(env: Env, id: BytesN<32>) -> bool {
        load_op(&env, &id).is_some()
    }

    /// Whether the operation exists and has not run yet.
    pub fn is_operation_pending(env: Env, id: BytesN<32>) -> bool {
        match load_op(&env, &id) {
            Some(op) => !op.done,
            None => false,
        }
    }

    /// Whether the operation may run now: pending, its delay elapsed, and
    /// its predecessor done.
    pub fn is_operation_ready(env: Env, id: BytesN<32>) -> bool {
        let op = match load_op(&env, &id) {
            Some(op) => op,
            None => return false,
        };
        if op.done {
            return false;
        }
        if env.ledger().timestamp() < op.ready_at {
            return false;
        }
        is_no_predecessor(&op.predecessor) || is_done(&env, &op.predecessor)
    }

    /// Whether the operation has run.
    pub fn is_operation_done(env: Env, id: BytesN<32>) -> bool {
        is_done(&env, &id)
    }

    /// The scheduled operation, if there is one.
    pub fn get_operation(env: Env, id: BytesN<32>) -> Option<Operation> {
        load_op(&env, &id)
    }

    /// The smallest delay an operation may be scheduled with.
    pub fn get_min_delay(env: Env) -> u64 {
        min_delay(&env)
    }

    /// Whether an address holds a role.
    pub fn has_role(env: Env, role: Role, account: Address) -> bool {
        has_role(&env, role, &account)
    }

    /// Whether anyone may execute a ready operation.
    pub fn is_open_executor(env: Env) -> bool {
        open_executor(&env)
    }

    /// The id of the operation made of `calls`, `predecessor` and `salt`.
    pub fn hash_operation(
        env: Env,
        calls: Vec<Call>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
    ) -> BytesN<32> {
        hash_operation(&env, &calls, &predecessor, &salt)
    }
}
