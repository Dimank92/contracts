#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
    Symbol, Vec,
};

#[cfg(test)]
mod test;

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    MinDelay,
    GracePeriod,
    Op(u64),
    OpIds,
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpStatus {
    Scheduled,
    Executed,
    Cancelled,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Operation {
    pub target: Address,
    pub function: Symbol,
    pub args: Vec<soroban_sdk::Val>,
    pub ready_at: u64,
    pub expires_at: u64,
    pub status: OpStatus,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    OpNotFound = 4,
    OpAlreadyExists = 5,
    OpNotReady = 6,
    OpExpired = 7,
    OpNotScheduled = 8,
    InvalidDelay = 9,
    InvalidGracePeriod = 10,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

const EVT_SCHEDULED: Symbol = symbol_short!("scheduled");
const EVT_EXECUTED: Symbol = symbol_short!("executed");
const EVT_CANCELLED: Symbol = symbol_short!("cancelled");

// ---------------------------------------------------------------------------
// Client interface (for cross-contract calls)
// ---------------------------------------------------------------------------

#[contractclient(name = "TimelockClient")]
pub trait TimelockInterface {
    fn schedule(
        env: Env,
        caller: Address,
        op_id: u64,
        target: Address,
        function: Symbol,
        args: Vec<soroban_sdk::Val>,
    ) -> u64;
    fn execute(env: Env, caller: Address, op_id: u64);
    fn cancel(env: Env, caller: Address, op_id: u64);
    fn is_done(env: Env, op_id: u64) -> bool;
    fn get_min_delay(env: Env) -> u64;
    fn get_grace_period(env: Env) -> u64;
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct Timelock;

#[contractimpl]
impl Timelock {
    /// Initialize the timelock with an admin, a minimum delay (seconds), and a
    /// grace period (seconds) after which a scheduled op expires.
    pub fn initialize(
        env: Env,
        admin: Address,
        min_delay: u64,
        grace_period: u64,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        if min_delay == 0 {
            return Err(Error::InvalidDelay);
        }
        if grace_period == 0 {
            return Err(Error::InvalidGracePeriod);
        }
        admin.require_auth();

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::MinDelay, &min_delay);
        env.storage()
            .instance()
            .set(&DataKey::GracePeriod, &grace_period);
        env.storage()
            .instance()
            .set(&DataKey::OpIds, &Vec::<u64>::new(&env));
        Ok(())
    }

    /// Schedule an operation. The caller must be the admin. The `op_id` is
    /// caller-provided so that an upstream contract (e.g. the governor) can
    /// use its own identifier (such as a proposal id) as the timelock op id.
    /// Returns the op id.
    pub fn schedule(
        env: Env,
        caller: Address,
        op_id: u64,
        target: Address,
        function: Symbol,
        args: Vec<soroban_sdk::Val>,
    ) -> Result<u64, Error> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let key = DataKey::Op(op_id);
        if env.storage().persistent().has(&key) {
            return Err(Error::OpAlreadyExists);
        }

        let min_delay: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MinDelay)
            .ok_or(Error::NotInitialized)?;
        let grace_period: u64 = env
            .storage()
            .instance()
            .get(&DataKey::GracePeriod)
            .ok_or(Error::NotInitialized)?;

        let now = env.ledger().timestamp();
        let ready_at = now.saturating_add(min_delay);
        let expires_at = ready_at.saturating_add(grace_period);

        let op = Operation {
            target: target.clone(),
            function: function.clone(),
            args,
            ready_at,
            expires_at,
            status: OpStatus::Scheduled,
        };
        env.storage().persistent().set(&key, &op);

        let mut ids: Vec<u64> = env
            .storage()
            .instance()
            .get(&DataKey::OpIds)
            .unwrap_or_else(|| Vec::new(&env));
        ids.push_back(op_id);
        env.storage().instance().set(&DataKey::OpIds, &ids);

        env.events().publish(
            (EVT_SCHEDULED, caller),
            (op_id, target, function, ready_at, expires_at),
        );

        Ok(op_id)
    }

    /// Execute a scheduled operation. Only the admin may trigger execution.
    /// The operation must be scheduled, past its ready time, and not expired.
    pub fn execute(env: Env, caller: Address, op_id: u64) -> Result<(), Error> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let key = DataKey::Op(op_id);
        let mut op: Operation = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::OpNotFound)?;

        if op.status != OpStatus::Scheduled {
            return Err(Error::OpNotScheduled);
        }

        let now = env.ledger().timestamp();
        if now < op.ready_at {
            return Err(Error::OpNotReady);
        }
        if now > op.expires_at {
            return Err(Error::OpExpired);
        }

        op.status = OpStatus::Executed;
        env.storage().persistent().set(&key, &op);

        env.events()
            .publish((EVT_EXECUTED, caller), (op_id, op.target.clone()));

        // Perform the actual cross-contract call.
        let _: soroban_sdk::Val = env.invoke_contract(
            &op.target,
            &op.function,
            op.args.clone(),
        );

        Ok(())
    }

    /// Cancel a scheduled operation. Only the admin may cancel.
    pub fn cancel(env: Env, caller: Address, op_id: u64) -> Result<(), Error> {
        caller.require_auth();
        Self::require_admin(&env, &caller)?;

        let key = DataKey::Op(op_id);
        let mut op: Operation = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(Error::OpNotFound)?;

        if op.status != OpStatus::Scheduled {
            return Err(Error::OpNotScheduled);
        }

        op.status = OpStatus::Cancelled;
        env.storage().persistent().set(&key, &op);

        env.events().publish((EVT_CANCELLED, caller), op_id);
        Ok(())
    }

    /// Returns true if the operation has been executed.
    pub fn is_done(env: Env, op_id: u64) -> bool {
        match env
            .storage()
            .persistent()
            .get::<DataKey, Operation>(&DataKey::Op(op_id))
        {
            Some(op) => op.status == OpStatus::Executed,
            None => false,
        }
    }

    /// Returns the configured minimum delay.
    pub fn get_min_delay(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MinDelay)
            .unwrap_or(0)
    }

    /// Returns the configured grace period.
    pub fn get_grace_period(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::GracePeriod)
            .unwrap_or(0)
    }

    /// Returns the stored operation, if any.
    pub fn get_operation(env: Env, op_id: u64) -> Option<Operation> {
        env.storage().persistent().get(&DataKey::Op(op_id))
    }

    /// Returns the admin address.
    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Admin)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn require_admin(env: &Env, caller: &Address) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        if &admin != caller {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
}