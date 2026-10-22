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

use soroban_sdk::token::{self, Interface as _};
use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, String, Symbol};

#[cfg(test)]
mod test;

mod error;
mod storage;

use error::Error;
use storage::{AllowanceKey, AllowanceValue, DataKey};

#[contract]
pub struct LpToken;

#[contractimpl]
impl LpToken {
    /// Pins `pool` as the only address that may mint and burn, and sets
    /// the token metadata. Requires the pool's own authorization so a
    /// front-runner can't claim a freshly deployed token for a pool that
    /// never agreed to it.
    ///
    /// Idempotent-safe: a second call fails with `AlreadyInitialized`
    /// rather than resetting the pool pin. Rotating the pool requires the
    /// explicit, admin-gated `set_pool` path.
    pub fn initialize(env: Env, pool: Address, decimal: u32, name: String, symbol: String) {
        if env.storage().instance().has(&DataKey::Pool) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        pool.require_auth();
        if decimal > u8::MAX.into() {
            panic_with_error!(&env, Error::InvalidDecimals);
        }
        storage::extend_instance(&env);
        let instance = env.storage().instance();
        instance.set(&DataKey::Pool, &pool);
        instance.set(&DataKey::Decimals, &decimal);
        instance.set(&DataKey::Name, &name);
        instance.set(&DataKey::Symbol, &symbol);
    }

    /// Rotates the pinned pool to `new_pool`. Admin-gated: the current
    /// pool must authorize the rotation. This is the only way the pin can
    /// change after `initialize`.
    pub fn set_pool(env: Env, new_pool: Address) {
        let current = Self::pool(env.clone());
        current.require_auth();
        storage::extend_instance(&env);
        env.storage().instance().set(&DataKey::Pool, &new_pool);
        env.events()
            .publish((Symbol::new(&env, "set_pool"), current), new_pool);
    }

    /// Mints `amount` new shares to `to`. Only the pinned pool.
    pub fn mint(env: Env, to: Address, amount: i128) {
        check_nonnegative(&env, amount);
        let pool = Self::pool(env.clone());
        pool.require_auth();
        storage::extend_instance(&env);
        receive(&env, &to, amount);
        env.events()
            .publish((Symbol::new(&env, "mint"), pool, to), amount);
    }

    /// Burns `amount` shares from `from`. Only the pinned pool.
    pub fn burn(env: Env, from: Address, amount: i128) {
        check_nonnegative(&env, amount);
        let pool = Self::pool(env.clone());
        pool.require_auth();
        storage::extend_instance(&env);
        spend(&env, &from, amount);
        env.events()
            .publish((Symbol::new(&env, "burn"), pool, from), amount);
    }

    pub fn pool(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Pool)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotInitialized))
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
        check_spender(&env, &spender);
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
        // A self-transfer is a no-op on balances but still emits the event.
        if from != to {
            spend(&env, &from, amount);
            receive(&env, &to, amount);
        }
        env.events()
            .publish((Symbol::new(&env, "transfer"), from, to), amount);
    }

    fn transfer_from(env: Env, spender: Address, from: Address, to: Address, amount: i128) {
        spender.require_auth();
        check_nonnegative(&env, amount);
        storage::extend_instance(&env);
        // Consume the allowance first; an allowance exactly equal to the
        // amount is sufficient and leaves a zero balance behind.
        spend_allowance(&env, &from, &spender, amount);
        if from != to {
            spend(&env, &from, amount);
            receive(&env, &to, amount);
        }
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
        storage::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::Decimals)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotInitialized))
    }

    fn name(env: Env) -> String {
        storage::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::Name)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotInitialized))
    }

    fn symbol(env: Env) -> String {
        storage::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::Symbol)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NotInitialized))
    }
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

/// Approving the zero address is rejected: a zero-address spender can
/// never authorize a `transfer_from`, so the allowance would be dead
/// weight and is almost always a caller bug.
fn check_spender(env: &Env, spender: &Address) {
    if *spender == Address::from_string(&String::from_str(env, "")) {
        panic_with_error!(env, Error::InvalidSpender);
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
