#![cfg(test)]

//! A minimal timelock for the governor's tests, implementing the subset
//! in `interfaces::Timelock` plus `execute` / `get_operation`. The real
//! timelock crate's sources are incomplete on main (README "Known gaps").

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Symbol, Val, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum MockTimelockError {
    NotInitialized = 1,
    UnknownOperation = 2,
    NotReady = 3,
    Expired = 4,
    Unauthorized = 5,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Operation {
    pub targets: Vec<Address>,
    pub fns: Vec<Symbol>,
    pub args: Vec<Vec<Val>>,
    pub ready_at: u64,
    pub done: bool,
}

#[contracttype]
enum Key {
    Proposer,
    Guardian,
    MinDelay,
    Grace,
    Op(BytesN<32>),
}

#[contract]
pub struct MockTimelock;

fn get<V: soroban_sdk::TryFromVal<Env, Val>>(env: &Env, key: &Key) -> Result<V, MockTimelockError> {
    env.storage()
        .instance()
        .get(key)
        .ok_or(MockTimelockError::NotInitialized)
}

#[contractimpl]
impl MockTimelock {
    pub fn initialize(env: Env, proposer: Address, guardian: Address, min_delay: u64, grace: u64) {
        let s = env.storage().instance();
        s.set(&Key::Proposer, &proposer);
        s.set(&Key::Guardian, &guardian);
        s.set(&Key::MinDelay, &min_delay);
        s.set(&Key::Grace, &grace);
    }

    pub fn schedule(
        env: Env,
        op_id: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
    ) -> Result<u64, MockTimelockError> {
        get::<Address>(&env, &Key::Proposer)?.require_auth();
        let ready_at = env.ledger().timestamp() + delay;
        env.storage().persistent().set(
            &Key::Op(op_id),
            &Operation {
                targets,
                fns,
                args,
                ready_at,
                done: false,
            },
        );
        Ok(ready_at)
    }

    /// Permissionless once ready and before the grace period ends.
    pub fn execute(env: Env, op_id: BytesN<32>) -> Result<(), MockTimelockError> {
        let key = Key::Op(op_id);
        let mut op: Operation = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(MockTimelockError::UnknownOperation)?;
        let now = env.ledger().timestamp();
        if op.done || now < op.ready_at {
            return Err(MockTimelockError::NotReady);
        }
        if now > op.ready_at + get::<u64>(&env, &Key::Grace)? {
            return Err(MockTimelockError::Expired);
        }
        op.done = true;
        env.storage().persistent().set(&key, &op);
        for i in 0..op.targets.len() {
            let (Some(target), Some(f), Some(a)) =
                (op.targets.get(i), op.fns.get(i), op.args.get(i))
            else {
                return Err(MockTimelockError::UnknownOperation);
            };
            env.invoke_contract::<Val>(&target, &f, a);
        }
        Ok(())
    }

    /// The proposer or the guardian.
    pub fn cancel(env: Env, caller: Address, op_id: BytesN<32>) -> Result<(), MockTimelockError> {
        caller.require_auth();
        if caller != get::<Address>(&env, &Key::Proposer)?
            && caller != get::<Address>(&env, &Key::Guardian)?
        {
            return Err(MockTimelockError::Unauthorized);
        }
        env.storage().persistent().remove(&Key::Op(op_id));
        Ok(())
    }

    pub fn get_operation(env: Env, op_id: BytesN<32>) -> Option<Operation> {
        env.storage().persistent().get(&Key::Op(op_id))
    }

    pub fn is_done(env: Env, op_id: BytesN<32>) -> bool {
        Self::get_operation(env, op_id)
            .map(|op| op.done)
            .unwrap_or(false)
    }

    pub fn get_min_delay(env: Env) -> Result<u64, MockTimelockError> {
        get(&env, &Key::MinDelay)
    }

    pub fn get_grace_period(env: Env) -> Result<u64, MockTimelockError> {
        get(&env, &Key::Grace)
    }
}
