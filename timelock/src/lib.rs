#![no_std]

//! Delayed-execution admin (OpenZeppelin TimelockController style). See
//! README "`timelock` reference".
//!
//! Predecessor ordering: an operation may name a `predecessor` id that must
//! have executed first. `schedule` walks the predecessor chain (at most
//! `MAX_CHAIN_DEPTH` hops) and rejects unknown, cancelled, cyclic or too
//! deep chains; `execute` re-checks the chain so cancelling a predecessor
//! leaves its dependents explicitly non-executable. Ids commit to their
//! predecessor, so a true cycle needs a sha256 fixed point; the walk is the
//! defence in depth that keeps that an invariant of the code, not of the hash.

mod error;
mod events;
mod migrations;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contractimpl, panic_with_error, symbol_short, xdr::ToXdr, Address, Bytes, BytesN,
    Env, Val, Vec,
};

pub use error::Error;
pub use types::{Call, DataKey, Operation, Role, Status, MAX_CHAIN_DEPTH};

#[contract]
pub struct Timelock;

fn op(env: &Env, id: &BytesN<32>) -> Option<Operation> {
    env.storage().persistent().get(&DataKey::Op(id.clone()))
}

fn put(env: &Env, id: &BytesN<32>, o: &Operation) {
    env.storage().persistent().set(&DataKey::Op(id.clone()), o);
}

fn has_role(env: &Env, role: Role, account: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::Role(role, account.clone()))
        .unwrap_or(false)
}

fn set_role(env: &Env, role: Role, account: &Address, granted: bool) {
    let key = DataKey::Role(role, account.clone());
    if granted {
        env.storage().persistent().set(&key, &true);
    } else {
        env.storage().persistent().remove(&key);
    }
    events::role_changed(env, role, account, granted);
}

fn require_role(env: &Env, role: Role, account: &Address) {
    account.require_auth();
    if !has_role(env, role, account) {
        panic_with_error!(env, Error::Unauthorized);
    }
}

fn min_delay(env: &Env) -> u64 {
    env.storage().instance().get(&DataKey::MinDelay).unwrap()
}

fn hash(
    env: &Env,
    calls: &Vec<Call>,
    predecessor: &Option<BytesN<32>>,
    salt: &BytesN<32>,
) -> BytesN<32> {
    let bytes: Bytes = (calls.clone(), predecessor.clone(), salt.clone()).to_xdr(env);
    env.crypto().sha256(&bytes).into()
}

/// Walks `start`'s predecessor chain. `forbid` is an id that must not
/// appear in it (the operation being scheduled). Every hop must exist and
/// not be cancelled. Returns whether `start` itself is Done.
fn check_chain(env: &Env, start: &BytesN<32>, forbid: Option<&BytesN<32>>) -> bool {
    let mut cur = start.clone();
    let mut start_done = false;
    for depth in 0..MAX_CHAIN_DEPTH {
        if forbid == Some(&cur) {
            panic_with_error!(env, Error::PredecessorCycle);
        }
        let o = match op(env, &cur) {
            Some(o) => o,
            None => panic_with_error!(env, Error::ActionNotFound),
        };
        match o.status {
            Status::Cancelled => panic_with_error!(env, Error::PredecessorCancelled),
            Status::Done if depth == 0 => start_done = true,
            _ => {}
        }
        match o.predecessor.get(0) {
            Some(p) => cur = p,
            None => return start_done,
        }
    }
    panic_with_error!(env, Error::PredecessorChainTooDeep)
}

#[contractimpl]
impl Timelock {
    pub fn initialize(
        env: Env,
        min_delay: u64,
        proposers: Vec<Address>,
        executors: Vec<Address>,
        cancellers: Vec<Address>,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::MinDelay) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::MinDelay, &min_delay);
        env.storage()
            .instance()
            .set(&DataKey::OpenExecutor, &executors.is_empty());
        for a in proposers.iter() {
            set_role(&env, Role::Proposer, &a, true);
        }
        for a in executors.iter() {
            set_role(&env, Role::Executor, &a, true);
        }
        for a in cancellers.iter() {
            set_role(&env, Role::Canceller, &a, true);
        }
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
        Ok(())
    }

    pub fn schedule(
        env: Env,
        proposer: Address,
        calls: Vec<Call>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
        delay: u64,
    ) -> Result<BytesN<32>, Error> {
        migrations::require_current(&env);
        require_role(&env, Role::Proposer, &proposer);
        if calls.is_empty() {
            panic_with_error!(&env, Error::EmptyOperation);
        }
        if delay < min_delay(&env) {
            panic_with_error!(&env, Error::InsufficientDelay);
        }
        let id = hash(&env, &calls, &predecessor, &salt);
        if op(&env, &id).is_some() {
            panic_with_error!(&env, Error::AlreadyScheduled);
        }
        if let Some(p) = &predecessor {
            check_chain(&env, p, Some(&id));
        }
        let ready_at = env.ledger().timestamp() + delay;
        put(
            &env,
            &id,
            &Operation {
                status: Status::Pending,
                ready_at,
                predecessor: match &predecessor {
                    Some(p) => soroban_sdk::vec![&env, p.clone()],
                    None => Vec::new(&env),
                },
            },
        );
        events::scheduled(&env, &id, ready_at, &predecessor);
        Ok(id)
    }

    pub fn execute(
        env: Env,
        executor: Address,
        calls: Vec<Call>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
    ) -> Result<(), Error> {
        migrations::require_current(&env);
        let open: bool = env
            .storage()
            .instance()
            .get(&DataKey::OpenExecutor)
            .unwrap_or(false);
        if !open {
            require_role(&env, Role::Executor, &executor);
        }
        let id = hash(&env, &calls, &predecessor, &salt);
        let mut o = match op(&env, &id) {
            Some(o) if o.status == Status::Pending => o,
            _ => panic_with_error!(&env, Error::NotPending),
        };
        if o.ready_at > env.ledger().timestamp() {
            panic_with_error!(&env, Error::NotReady);
        }
        if let Some(p) = &predecessor {
            if !check_chain(&env, p, None) {
                panic_with_error!(&env, Error::PredecessorNotExecuted);
            }
        }
        // Mark done before dispatch: a failing call reverts the whole tx.
        o.status = Status::Done;
        put(&env, &id, &o);
        for c in calls.iter() {
            if c.target == env.current_contract_address() {
                Self::self_call(&env, &c.function, &c.args);
            } else {
                env.invoke_contract::<Val>(&c.target, &c.function, c.args.clone());
            }
        }
        events::executed(&env, &id);
        Ok(())
    }

    /// Cancelling leaves a tombstone: dependents fail with
    /// `PredecessorCancelled` instead of sitting pending forever.
    pub fn cancel(env: Env, canceller: Address, id: BytesN<32>) -> Result<(), Error> {
        require_role(&env, Role::Canceller, &canceller);
        let mut o = match op(&env, &id) {
            Some(o) if o.status == Status::Pending => o,
            _ => panic_with_error!(&env, Error::NotPending),
        };
        o.status = Status::Cancelled;
        put(&env, &id, &o);
        events::cancelled(&env, &id);
        Ok(())
    }

    pub fn hash_operation(
        env: Env,
        calls: Vec<Call>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
    ) -> BytesN<32> {
        hash(&env, &calls, &predecessor, &salt)
    }

    /// 0 unknown or cancelled, 1 done, else the ready-at timestamp.
    pub fn get_timestamp(env: Env, id: BytesN<32>) -> u64 {
        match op(&env, &id) {
            Some(o) => match o.status {
                Status::Pending => o.ready_at,
                Status::Done => 1,
                Status::Cancelled => 0,
            },
            None => 0,
        }
    }

    pub fn is_operation(env: Env, id: BytesN<32>) -> bool {
        Self::get_timestamp(env, id) > 0
    }

    pub fn is_operation_pending(env: Env, id: BytesN<32>) -> bool {
        Self::get_timestamp(env, id) > 1
    }

    pub fn is_operation_ready(env: Env, id: BytesN<32>) -> bool {
        let t = Self::get_timestamp(env.clone(), id);
        t > 1 && t <= env.ledger().timestamp()
    }

    pub fn is_operation_done(env: Env, id: BytesN<32>) -> bool {
        Self::get_timestamp(env, id) == 1
    }

    pub fn get_min_delay(env: Env) -> u64 {
        min_delay(&env)
    }

    pub fn has_role(env: Env, role: Role, account: Address) -> bool {
        has_role(&env, role, &account)
    }

    pub fn is_open_executor(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::OpenExecutor)
            .unwrap_or(false)
    }
}

impl Timelock {
    /// Soroban forbids re-entry, so calls targeting the timelock itself are
    /// dispatched here; they are only reachable through a ready operation.
    fn self_call(env: &Env, func: &soroban_sdk::Symbol, args: &Vec<Val>) {
        fn arg<T: soroban_sdk::TryFromVal<Env, Val>>(env: &Env, args: &Vec<Val>, i: u32) -> T {
            match args.get(i).and_then(|v| T::try_from_val(env, &v).ok()) {
                Some(v) => v,
                None => panic_with_error!(env, Error::UnknownSelfCall),
            }
        }
        if *func == soroban_sdk::Symbol::new(env, "update_delay") {
            let new: u64 = arg(env, args, 0);
            let old = min_delay(env);
            env.storage().instance().set(&DataKey::MinDelay, &new);
            events::min_delay_changed(env, old, new);
        } else if *func == soroban_sdk::Symbol::new(env, "grant_role") {
            set_role(env, arg(env, args, 0), &arg::<Address>(env, args, 1), true);
        } else if *func == soroban_sdk::Symbol::new(env, "revoke_role") {
            set_role(env, arg(env, args, 0), &arg::<Address>(env, args, 1), false);
        } else if *func == soroban_sdk::Symbol::new(env, "set_open_executor") {
            let open: bool = arg(env, args, 0);
            env.storage().instance().set(&DataKey::OpenExecutor, &open);
        } else if *func == symbol_short!("migrate") {
            migrations::self_migrate(env, arg(env, args, 0), arg(env, args, 1));
        } else {
            panic_with_error!(env, Error::UnknownSelfCall);
        }
    }
}
