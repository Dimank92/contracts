#![no_std]

//! Zenith Vault — a per-tag escrow ledger for a single token.
//!
//! Built to close a gap discovered while testing options_market: that
//! contract held every writer's collateral and every buyer's premium in
//! one undifferentiated token balance, with no accounting of which
//! balance is actually earmarked for which position. This contract is
//! that missing accounting layer — deposit/withdraw against a caller-
//! defined `tag` (options_market uses its series_id), so "how much is
//! earmarked for tag 7" is always answerable instead of inferred from the
//! token contract's raw balance, and a withdrawal against one tag can
//! never draw down another tag's funds.
//!
//! `deposit` is open to anyone (it only ever adds funds); everything that
//! moves funds out or between tags is admin-gated. In the protocol's
//! deployment the admin is the options_market contract itself.
//!
//! Every admin function is a thin entrypoint over an `x_inner(env, Auth,
//! ...)` that starts with `zenith_common::require_admin_or_multisig`; the
//! `_via_multisig` twins only differ in the `Auth` they pass.

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, BytesN, Env, Vec};
use zenith_common::{self as common, ActionClass, Auth};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_adversaries;

mod error;
mod events;
mod migrations;
mod token_validation;
mod ttl;
mod types;

use error::Error;
use token_validation::{TokenError, TokenTransferGuard};
pub use types::DataKey;

fn gate(env: &Env, auth: &Auth, class: ActionClass) {
    common::require_admin_or_multisig(env, &DataKey::Admin, auth, class, Error::Unauthorized);
}

fn require_not_paused(env: &Env) {
    common::require_not_paused(env, &DataKey::Paused, Error::ContractPaused);
}

fn token_address(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Token).unwrap()
}

fn total_escrowed(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalEscrowed)
        .unwrap_or(0)
}

fn set_total_escrowed(env: &Env, total: i128) {
    env.storage()
        .instance()
        .set(&DataKey::TotalEscrowed, &total);
}

fn checked(env: &Env, value: Option<i128>) -> i128 {
    value.unwrap_or_else(|| panic_with_error!(env, Error::InvalidAmount))
}

/// Debits `amount` from `tag`, failing if the tag's own balance is short
/// — whatever other tags hold is irrelevant.
fn debit(env: &Env, tag: u64, amount: i128) {
    let key = DataKey::Escrow(tag);
    let balance: i128 = ttl::get_persistent(env, &key).unwrap_or(0);
    if balance < amount {
        panic_with_error!(env, Error::InsufficientEscrowBalance);
    }
    ttl::set_persistent(env, &key, &(balance - amount));
}

fn credit(env: &Env, tag: u64, amount: i128) {
    let key = DataKey::Escrow(tag);
    let balance: i128 = ttl::get_persistent(env, &key).unwrap_or(0);
    ttl::set_persistent(env, &key, &checked(env, balance.checked_add(amount)));
}

#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    pub fn initialize(env: Env, admin: Address, token: Address) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        set_total_escrowed(&env, 0);
        common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    // ── Admin: transfer (two-step) ────────────────────────────────────────────

    /// Step one of an admin transfer; see `accept_admin`.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        Self::transfer_admin_inner(&env, Auth::Admin, new_admin);
    }

    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        new_admin: Address,
    ) {
        Self::transfer_admin_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            new_admin,
        );
    }

    /// Step two: the pending admin takes over.
    pub fn accept_admin(env: Env) {
        ttl::extend_instance(&env);
        let (old, new) = common::accept_admin(&env, &DataKey::Admin, Error::NoPendingAdmin);
        events::admin_transferred(&env, old, new);
    }

    pub fn get_pending_admin(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        common::pending_admin(&env)
    }

    // ── Admin: pause ──────────────────────────────────────────────────────────

    /// Blocks deposit, withdraw and transfer_tag.
    pub fn pause(env: Env) {
        Self::set_paused_inner(&env, Auth::Admin, true);
    }

    pub fn unpause(env: Env) {
        Self::set_paused_inner(&env, Auth::Admin, false);
    }

    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        Self::set_paused_inner(&env, Auth::Multisig(multisig_contract, action_id), true);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        Self::set_paused_inner(&env, Auth::Multisig(multisig_contract, action_id), false);
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        common::is_paused(&env, &DataKey::Paused)
    }

    // ── Escrow ────────────────────────────────────────────────────────────────

    /// Pulls `amount` from `from` and credits it to `tag`. The vault
    /// measures its own balance before and after the transfer and rejects
    /// the deposit (`DeltaMismatch`) unless at least `amount` actually
    /// arrived, so fee-on-transfer, no-op or lying tokens can't create
    /// escrow that isn't there.
    pub fn deposit(env: Env, from: Address, tag: u64, amount: i128) {
        ttl::extend_instance(&env);
        from.require_auth();
        require_not_paused(&env);
        migrations::require_current(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let token_address = token_address(&env);
        match TokenTransferGuard::safe_inbound_transfer(
            &env,
            &token_address,
            &from,
            &env.current_contract_address(),
            amount,
        ) {
            Ok(_) => {}
            Err(TokenError::ZeroTransfer) => panic_with_error!(&env, Error::InvalidAmount),
            Err(_) => panic_with_error!(&env, Error::DeltaMismatch),
        }

        credit(&env, tag, amount);
        set_total_escrowed(
            &env,
            checked(&env, total_escrowed(&env).checked_add(amount)),
        );
        events::deposited(&env, from, tag, amount);
    }

    /// Pays `amount` of `tag`'s own escrow to `to`. Admin only.
    pub fn withdraw(env: Env, tag: u64, to: Address, amount: i128) {
        Self::withdraw_inner(&env, Auth::Admin, tag, to, amount);
    }

    pub fn withdraw_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        tag: u64,
        to: Address,
        amount: i128,
    ) {
        Self::withdraw_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            tag,
            to,
            amount,
        );
    }

    /// Re-earmarks `amount` from one tag to another; no tokens move.
    pub fn transfer_tag(env: Env, from_tag: u64, to_tag: u64, amount: i128) {
        Self::transfer_tag_inner(&env, Auth::Admin, from_tag, to_tag, amount);
    }

    pub fn transfer_tag_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        from_tag: u64,
        to_tag: u64,
        amount: i128,
    ) {
        Self::transfer_tag_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            from_tag,
            to_tag,
            amount,
        );
    }

    /// Recovers tokens sent straight to the vault (bypassing `deposit`):
    /// everything above the ledger's total. Tagged escrow is untouched.
    pub fn sweep_untagged(env: Env, to: Address) -> i128 {
        Self::sweep_untagged_inner(&env, Auth::Admin, to)
    }

    pub fn sweep_untagged_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        to: Address,
    ) -> i128 {
        Self::sweep_untagged_inner(&env, Auth::Multisig(multisig_contract, action_id), to)
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn balance_of(env: Env, tag: u64) -> i128 {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::Escrow(tag)).unwrap_or(0)
    }

    pub fn get_total_escrowed(env: Env) -> i128 {
        ttl::extend_instance(&env);
        total_escrowed(&env)
    }

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        common::get_admin(&env, &DataKey::Admin)
    }

    pub fn get_token(env: Env) -> Address {
        ttl::extend_instance(&env);
        token_address(&env)
    }
}

/// Admin bodies shared by each entrypoint and its `_via_multisig` twin.
/// Auth is resolved first, before any other state is read.
impl Vault {
    fn transfer_admin_inner(env: &Env, auth: Auth, new_admin: Address) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Critical);
        common::propose_admin(env, &DataKey::Admin, &new_admin);
        events::admin_transfer_proposed(env, common::get_admin(env, &DataKey::Admin), new_admin);
    }

    fn set_paused_inner(env: &Env, auth: Auth, paused: bool) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Emergency);
        common::set_paused(env, &DataKey::Paused, paused);
        if paused {
            events::paused(env);
        } else {
            events::unpaused(env);
        }
    }

    fn withdraw_inner(env: &Env, auth: Auth, tag: u64, to: Address, amount: i128) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Critical);
        require_not_paused(env);
        migrations::require_current(env);
        if amount <= 0 {
            panic_with_error!(env, Error::InvalidAmount);
        }

        debit(env, tag, amount);
        set_total_escrowed(env, checked(env, total_escrowed(env).checked_sub(amount)));
        token::Client::new(env, &token_address(env)).transfer(
            &env.current_contract_address(),
            &to,
            &amount,
        );
        events::withdrawn(env, to, tag, amount);
    }

    fn transfer_tag_inner(env: &Env, auth: Auth, from_tag: u64, to_tag: u64, amount: i128) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Critical);
        require_not_paused(env);
        migrations::require_current(env);
        if amount <= 0 {
            panic_with_error!(env, Error::InvalidAmount);
        }

        debit(env, from_tag, amount);
        credit(env, to_tag, amount);
        // TotalEscrowed is unaffected — nothing entered or left the vault,
        // only which tag it's earmarked under changed.
        events::tag_transferred(env, from_tag, to_tag, amount);
    }

    fn sweep_untagged_inner(env: &Env, auth: Auth, to: Address) -> i128 {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Standard);
        migrations::require_current(env);

        let token_client = token::Client::new(env, &token_address(env));
        let actual_balance = token_client.balance(&env.current_contract_address());
        let untagged = actual_balance.saturating_sub(total_escrowed(env));
        if untagged <= 0 {
            panic_with_error!(env, Error::NoUntaggedFunds);
        }

        token_client.transfer(&env.current_contract_address(), &to, &untagged);
        events::swept_untagged(env, to, untagged);
        untagged
    }
}
