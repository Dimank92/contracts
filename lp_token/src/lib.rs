#![no_std]

//! Zenith LP share token — a SEP-41 token whose supply only the pinned
//! `lp_pool` contract can mint.
//!
//! Implements `soroban_sdk::token::Interface` directly rather than
//! building on OpenZeppelin Stellar's `fungible` module: the interface is
//! small, every other contract here pins soroban-sdk 21, and a direct
//! implementation keeps the audited surface to this one file with no
//! extra dependency. Storage and event layout follow the reference
//! `soroban-examples/token` contract:
//!
//! - balances are persistent entries, extended whenever touched;
//! - allowances are temporary entries whose TTL runs to their
//!   `expiration_ledger`, so an expired allowance reads as zero and is
//!   reclaimed by the network without anyone paying to delete it;
//! - events use the SEP-41 topics: `approve`, `transfer`, `mint`, `burn`.
//!   These keep the standard layout (no schema-version topic) so wallets
//!   and indexers recognize them; only `initialized` follows Zenith's
//!   event schema v2 (docs/events.md).
//!
//! The `token::Interface` functions keep their SEP-41 signatures, so they
//! fail with `panic_with_error!` (still a typed `Error`); every other
//! entrypoint returns `Result` (docs/errors.md).

use soroban_sdk::token::{self, Interface as _};
use soroban_sdk::{
    contract, contractimpl, contracttype, panic_with_error, Address, Env, String, Symbol,
};
use zenith_common::events::topics;

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_events;

mod error;
mod storage;

pub use error::Error;
use storage::{AllowanceKey, AllowanceValue, DataKey};

zenith_common::contract_meta!();

/// Data of the `initialized` event (schema v2).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializedEvent {
    pub pool: Address,
    pub decimals: u32,
    pub name: String,
    pub symbol: String,
}

#[contract]
pub struct LpToken;

#[contractimpl]
impl LpToken {
    /// Pins `pool` as the only address that may mint. Requires the
    /// pool's own authorization so a front-runner can't claim a freshly
    /// deployed token for a pool that never agreed to it.
    pub fn initialize(
        env: Env,
        pool: Address,
        decimal: u32,
        name: String,
        symbol: String,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Pool) {
            return Err(Error::AlreadyInitialized);
        }
        pool.require_auth();
        if decimal > u8::MAX.into() {
            return Err(Error::InvalidDecimals);
        }
        storage::extend_instance(&env);
        let instance = env.storage().instance();
        instance.set(&DataKey::Pool, &pool);
        instance.set(&DataKey::Decimals, &decimal);
        instance.set(&DataKey::Name, &name);
        instance.set(&DataKey::Symbol, &symbol);
        env.events().publish(
            topics(&env, "initialized"),
            InitializedEvent {
                pool,
                decimals: decimal,
                name,
                symbol,
            },
        );
        Ok(())
    }

    /// Mints `amount` new shares to `to`. Only the pinned pool.
    pub fn mint(env: Env, to: Address, amount: i128) -> Result<(), Error> {
        if amount < 0 {
            return Err(Error::NegativeAmount);
        }
        let pool = Self::pool(env.clone())?;
        pool.require_auth();
        storage::extend_instance(&env);
        let balance = storage::read_balance(&env, &to)
            .checked_add(amount)
            .ok_or(Error::Overflow)?;
        storage::write_balance(&env, &to, balance);
        env.events()
            .publish((Symbol::new(&env, "mint"), pool, to), amount);
        Ok(())
    }

    pub fn pool(env: Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&DataKey::Pool)
            .ok_or(Error::NotInitialized)
    }
}

#[contractimpl]
impl token::Interface for LpToken {
    fn allowance(env: Env, from: Address, spender: Address) -> i128 {
        storage::extend_instance(&env);
        read_allowance(&env, &from, &spender).amount
    }

    fn approve(env: Env, from: Address, spender: Address, amount: i128, expiration_ledger: u32) {
        from.require_auth();
        check_nonnegative(&env, amount);
        storage::extend_instance(&env);
        write_allowance(&env, &from, &spender, amount, expiration_ledger);
        env.events().publish(
            (Symbol::new(&env, "approve"), from, spender),
            (amount, expiration_ledger),
        );
    }

    fn balance(env: Env, id: Address) -> i128 {
        storage::extend_instance(&env);
        storage::read_balance(&env, &id)
    }

    fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        check_nonnegative(&env, amount);
        storage::extend_instance(&env);
        spend(&env, &from, amount);
        receive(&env, &to, amount);
        env.events()
            .publish((Symbol::new(&env, "transfer"), from, to), amount);
    }

    fn transfer_from(env: Env, spender: Address, from: Address, to: Address, amount: i128) {
        spender.require_auth();
        check_nonnegative(&env, amount);
        storage::extend_instance(&env);
        spend_allowance(&env, &from, &spender, amount);
        spend(&env, &from, amount);
        receive(&env, &to, amount);
        env.events()
            .publish((Symbol::new(&env, "transfer"), from, to), amount);
    }

    fn burn(env: Env, from: Address, amount: i128) {
        from.require_auth();
        check_nonnegative(&env, amount);
        storage::extend_instance(&env);
        spend(&env, &from, amount);
        env.events()
            .publish((Symbol::new(&env, "burn"), from), amount);
    }

    fn burn_from(env: Env, spender: Address, from: Address, amount: i128) {
        spender.require_auth();
        check_nonnegative(&env, amount);
        storage::extend_instance(&env);
        spend_allowance(&env, &from, &spender, amount);
        spend(&env, &from, amount);
        env.events()
            .publish((Symbol::new(&env, "burn"), from), amount);
    }

    fn decimals(env: Env) -> u32 {
        metadata(&env, &DataKey::Decimals)
    }

    fn name(env: Env) -> String {
        metadata(&env, &DataKey::Name)
    }

    fn symbol(env: Env) -> String {
        metadata(&env, &DataKey::Symbol)
    }
}

/// A metadata entry `initialize` wrote, or a typed `NotInitialized`.
fn metadata<V: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>>(env: &Env, key: &DataKey) -> V {
    env.storage()
        .instance()
        .get(key)
        .unwrap_or_else(|| panic_with_error!(env, Error::NotInitialized))
}

fn check_nonnegative(env: &Env, amount: i128) {
    if amount < 0 {
        panic_with_error!(env, Error::NegativeAmount);
    }
}

fn receive(env: &Env, to: &Address, amount: i128) {
    let balance = storage::read_balance(env, to)
        .checked_add(amount)
        .unwrap_or_else(|| panic_with_error!(env, Error::Overflow));
    storage::write_balance(env, to, balance);
}

fn spend(env: &Env, from: &Address, amount: i128) {
    let balance = storage::read_balance(env, from);
    if balance < amount {
        panic_with_error!(env, Error::InsufficientBalance);
    }
    storage::write_balance(env, from, balance - amount);
}

/// An allowance past its expiration ledger reads as zero, per SEP-41.
fn read_allowance(env: &Env, from: &Address, spender: &Address) -> AllowanceValue {
    let key = DataKey::Allowance(AllowanceKey {
        from: from.clone(),
        spender: spender.clone(),
    });
    match env.storage().temporary().get::<_, AllowanceValue>(&key) {
        Some(a) if a.expiration_ledger >= env.ledger().sequence() => a,
        _ => AllowanceValue {
            amount: 0,
            expiration_ledger: 0,
        },
    }
}

fn write_allowance(
    env: &Env,
    from: &Address,
    spender: &Address,
    amount: i128,
    expiration_ledger: u32,
) {
    let ledger = env.ledger().sequence();
    // A nonzero allowance must not already be expired; a zero one may use
    // any expiration (it's how allowances are cleared).
    if amount > 0 && expiration_ledger < ledger {
        panic_with_error!(env, Error::InvalidExpiration);
    }
    let key = DataKey::Allowance(AllowanceKey {
        from: from.clone(),
        spender: spender.clone(),
    });
    env.storage().temporary().set(
        &key,
        &AllowanceValue {
            amount,
            expiration_ledger,
        },
    );
    if amount > 0 {
        let live_for = expiration_ledger - ledger;
        env.storage()
            .temporary()
            .extend_ttl(&key, live_for, live_for);
    }
}

fn spend_allowance(env: &Env, from: &Address, spender: &Address, amount: i128) {
    let allowance = read_allowance(env, from, spender);
    if allowance.amount < amount {
        panic_with_error!(env, Error::InsufficientAllowance);
    }
    if amount > 0 {
        write_allowance(
            env,
            from,
            spender,
            allowance.amount - amount,
            allowance.expiration_ledger,
        );
    }
}
