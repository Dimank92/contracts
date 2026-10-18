#![no_std]

//! Zenith Vault — a per-(token, tag) escrow ledger.
//!
//! Minimal ledger for the options_market integration (#171): escrow is
//! keyed by `(token, series_id)` under a single integrator — the admin,
//! which is the options_market contract in the intended deployment — so
//! a withdrawal can never draw down more than was deposited under that
//! tag, whatever the vault's raw balance is from other tags. The
//! multi-token, multi-integrator `Tag` ledger in the README is the
//! separate vault rebuild (#164).

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env};

pub mod error;
pub mod events;
pub mod token_validation;
pub mod ttl;
mod types;

use error::Error;
use types::DataKey;

#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    /// Permissionless keeper entrypoint: extends the contract instance.
    pub fn bump(env: Env) {
        ttl::extend_instance(&env);
    }

    /// One-time setup. `admin` becomes the trusted integrator (e.g.
    /// the options_market contract address). `token` is the default
    /// (and only, in this simplified version) collateral token.
    pub fn initialize(env: Env, admin: Address, token: Address) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Paused, &false);
    }

    /// Transfers admin to a new address.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        ttl::extend_instance(&env);
        let admin = Self::admin(&env);
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Emergency stop.
    pub fn pause(env: Env) {
        ttl::extend_instance(&env);
        Self::admin(&env).require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        ttl::extend_instance(&env);
        Self::admin(&env).require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    /// Pulls `amount` of the vault's token from `from` and credits it
    /// to the `(token, series_id)` escrow tag. Returns the measured
    /// balance delta (what actually arrived).
    pub fn deposit(env: Env, from: Address, series_id: u64, amount: i128) -> i128 {
        ttl::extend_instance(&env);
        Self::require_not_paused(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let token_addr = Self::token(&env);
        let token_client = token::Client::new(&env, &token_addr);

        // Measure the balance delta (checks-effects-interactions).
        let vault_addr = env.current_contract_address();
        let before = token_client.balance(&vault_addr);
        from.require_auth();
        token_client.transfer(&from, &vault_addr, &amount);
        let after = token_client.balance(&vault_addr);
        let received = after - before;

        // Credit the escrow entry.
        let key = DataKey::Escrow(token_addr.clone(), series_id);
        let current: i128 = ttl::get_persistent(&env, &key).unwrap_or(0);
        ttl::set_persistent(&env, &key, &(current + received));

        // Update total escrowed.
        let total_key = DataKey::TotalEscrowed(token_addr.clone());
        let total: i128 = ttl::get_persistent(&env, &total_key).unwrap_or(0);
        ttl::set_persistent(&env, &total_key, &(total + received));

        events::deposited(&env, token_addr, series_id, from, received);
        received
    }

    /// Pays `amount` of `(token, series_id)`'s escrow to `to`.
    /// Requires the admin (integrator contract) to authorize.
    pub fn withdraw(env: Env, series_id: u64, to: Address, amount: i128) -> i128 {
        ttl::extend_instance(&env);
        Self::require_not_paused(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        // The admin (integrator contract) must authorize this withdrawal.
        Self::admin(&env).require_auth();

        let token_addr = Self::token(&env);
        let key = DataKey::Escrow(token_addr.clone(), series_id);
        let balance: i128 = ttl::get_persistent(&env, &key).unwrap_or(0);
        if balance < amount {
            panic_with_error!(&env, Error::InsufficientEscrowBalance);
        }

        // Check the vault's actual token balance can cover it.
        let token_client = token::Client::new(&env, &token_addr);
        let vault_balance = token_client.balance(&env.current_contract_address());
        if vault_balance < amount {
            panic_with_error!(&env, Error::InsufficientVaultBalance);
        }

        // Debit the escrow entry.
        ttl::set_persistent(&env, &key, &(balance - amount));

        // Update total escrowed.
        let total_key = DataKey::TotalEscrowed(token_addr.clone());
        let total: i128 = ttl::get_persistent(&env, &total_key).unwrap_or(0);
        ttl::set_persistent(&env, &total_key, &(total - amount));

        // Transfer to the recipient.
        token_client.transfer(&env.current_contract_address(), &to, &amount);

        events::withdrawn(&env, token_addr, series_id, to, amount);
        amount
    }

    /// Returns the escrow balance for `(token, series_id)`.
    pub fn balance_of(env: Env, series_id: u64) -> i128 {
        ttl::extend_instance(&env);
        let token_addr = Self::token(&env);
        let key = DataKey::Escrow(token_addr, series_id);
        ttl::get_persistent(&env, &key).unwrap_or(0)
    }

    // ── Views ──────────────────────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        Self::admin(&env)
    }

    pub fn get_token(env: Env) -> Address {
        ttl::extend_instance(&env);
        Self::token(&env)
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Paused).unwrap_or(false)
    }

    pub fn get_total_escrowed(env: Env, token: Address) -> i128 {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::TotalEscrowed(token)).unwrap_or(0)
    }
}

impl Vault {
    fn admin(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    fn token(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Token).unwrap()
    }

    fn require_not_paused(env: &Env) {
        let paused: bool = env.storage().instance().get(&DataKey::Paused).unwrap_or(false);
        if paused {
            panic_with_error!(env, Error::ContractPaused);
        }
    }
}
