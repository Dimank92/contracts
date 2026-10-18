#![no_std]

//! Zenith Protocol — Vault
//!
//! A per-(token, tag) escrow ledger for any number of allowlisted tokens and
//! integrator contracts. Each deposit/withdraw is scoped to a tag so one
//! integrator's funds can never be drawn down by another.
//!
//! Issue #193 adds:
//! - A **token allowlist** (`set_token_allowed`): only allowlisted tokens may
//!   be deposited.
//! - An **integrator registry** (`set_integrator`): every deposit is credited
//!   to a tag whose `owner` must be a registered integrator *and* must
//!   authorize the deposit, so one integrator can never deposit into
//!   another's namespace. An integrator calling the vault directly satisfies
//!   its own `require_auth` implicitly (invoker auth), which is how
//!   `options_market`-style contracts use it.
//!
//! Both gates go through one helper, `require_deposit_allowed`, shared by
//! `deposit` and `deposit_legacy` (where the tag owner is the admin, which
//! is trusted by definition, so only the token gate applies).
//!
//! **Removal never strands funds:** withdrawals are deliberately *not*
//! gated on either list. Removing a token or deregistering an integrator
//! only blocks further deposits; existing tagged balances stay withdrawable
//! by the tag owner (`withdraw` / `withdraw_legacy`) or by the multisig
//! (`withdraw_via_multisig`).

use soroban_sdk::{
    contract, contractimpl, panic_with_error, token, Address, Env, Vec,
};

mod error;
mod events;
pub mod token_validation;
pub mod types;

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_adversaries;

mod migrations;
mod ttl;

use error::Error;
use types::{DataKey, Tag};
use zenith_common::{require_admin_or_multisig, ActionClass, Auth};

// ─── helpers ─────────────────────────────────────────────────────────────────

fn require_initialized(env: &Env) {
    if !env.storage().instance().has(&DataKey::Admin) {
        panic_with_error!(env, Error::NotInitialized);
    }
}

fn require_admin(env: &Env) {
    require_initialized(env);
    zenith_common::get_admin(env, &DataKey::Admin).require_auth();
}

fn require_admin_via_multisig(env: &Env, multisig: Address, action_id: u64, class: ActionClass) {
    require_initialized(env);
    require_admin_or_multisig(
        env,
        &DataKey::Admin,
        &Auth::Multisig(multisig, action_id),
        class,
        Error::Unauthorized,
    );
}

fn is_token_allowed(env: &Env, token: &Address) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::TokenAllowed(token.clone()))
}

fn is_integrator(env: &Env, addr: &Address) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::Integrator(addr.clone()))
}

/// The single allowlist gate for every deposit path: `token` must be
/// allowlisted (`TokenNotAllowed`), and `integrator` — the tag owner —
/// must be registered (`UnregisteredIntegrator`) unless it is the admin,
/// which owns the legacy tags. Withdrawals deliberately skip this check;
/// see the crate docs.
fn require_deposit_allowed(env: &Env, token: &Address, integrator: &Address) {
    if !is_token_allowed(env, token) {
        panic_with_error!(env, Error::TokenNotAllowed);
    }
    let admin = zenith_common::get_admin(env, &DataKey::Admin);
    if integrator != &admin && !is_integrator(env, integrator) {
        panic_with_error!(env, Error::UnregisteredIntegrator);
    }
}

fn set_flag(env: &Env, key: DataKey, on: bool) {
    if on {
        ttl::set_persistent(env, &key, &true);
    } else {
        env.storage().persistent().remove(&key);
    }
}

fn set_token_allowed_inner(env: &Env, token: Address, allowed: bool) {
    set_flag(env, DataKey::TokenAllowed(token.clone()), allowed);
    events::token_allowed(env, token, allowed);
}

fn set_integrator_inner(env: &Env, integrator: Address, allowed: bool) {
    set_flag(env, DataKey::Integrator(integrator.clone()), allowed);
    events::integrator_set(env, integrator, allowed);
}

fn require_not_paused(env: &Env) {
    zenith_common::require_not_paused(env, &DataKey::Paused, Error::ContractPaused);
}

fn get_total_escrowed_raw(env: &Env, token: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalEscrowed(token.clone()))
        .unwrap_or(0i128)
}

fn set_total_escrowed(env: &Env, token: &Address, amount: i128) {
    env.storage()
        .instance()
        .set(&DataKey::TotalEscrowed(token.clone()), &amount);
}

fn get_escrow(env: &Env, token: &Address, tag: &Tag) -> i128 {
    ttl::get_persistent(env, &DataKey::Escrow(token.clone(), tag.clone())).unwrap_or(0i128)
}

fn set_escrow(env: &Env, token: &Address, tag: &Tag, amount: i128) {
    ttl::set_persistent(env, &DataKey::Escrow(token.clone(), tag.clone()), &amount);
}

// ─── contract ────────────────────────────────────────────────────────────────

#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    // ── lifecycle ──────────────────────────────────────────────────────────

    /// One-time setup. `token` becomes the default token and is
    /// automatically added to the allowlist. `max_pause_duration` (seconds,
    /// non-zero) is the maximum cumulative pause duration before the
    /// emergency-withdraw escape hatch opens (immutable after initialize).
    pub fn initialize(env: Env, admin: Address, token: Address, max_pause_duration: u64) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        if max_pause_duration == 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::MaxPauseDuration, &max_pause_duration);
        env.storage()
            .instance()
            .set(&DataKey::AccruedPauseDuration, &0u64);
        // Default token is allowlisted automatically
        env.storage()
            .persistent()
            .set(&DataKey::TokenAllowed(token.clone()), &true);
        ttl::extend_persistent(&env, &DataKey::TokenAllowed(token));
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    // ── keeper ─────────────────────────────────────────────────────────────

    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    // ── admin: transfer ────────────────────────────────────────────────────

    pub fn transfer_admin(env: Env, new_admin: Address) {
        ttl::extend_instance(&env);
        require_admin(&env);
        let old = zenith_common::set_admin(&env, &DataKey::Admin, &new_admin);
        events::admin_transferred(&env, old, new_admin);
    }

    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        ttl::extend_instance(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Critical,
            Error::Unauthorized,
        );
        let old = zenith_common::set_admin(&env, &DataKey::Admin, &new_admin);
        events::admin_transferred(&env, old, new_admin);
    }

    // ── admin: pause ───────────────────────────────────────────────────────

    pub fn pause(env: Env) {
        ttl::extend_instance(&env);
        require_admin(&env);
        if !zenith_common::is_paused(&env, &DataKey::Paused) {
            env.storage()
                .instance()
                .set(&DataKey::PausedAt, &env.ledger().timestamp());
            zenith_common::set_paused(&env, &DataKey::Paused, true);
            events::paused(&env);
        }
    }

    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        ttl::extend_instance(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Emergency,
            Error::Unauthorized,
        );
        if !zenith_common::is_paused(&env, &DataKey::Paused) {
            env.storage()
                .instance()
                .set(&DataKey::PausedAt, &env.ledger().timestamp());
            zenith_common::set_paused(&env, &DataKey::Paused, true);
            events::paused(&env);
        }
    }

    pub fn unpause(env: Env) {
        ttl::extend_instance(&env);
        require_admin(&env);
        if zenith_common::is_paused(&env, &DataKey::Paused) {
            // Accumulate elapsed pause time
            let paused_at: u64 = env
                .storage()
                .instance()
                .get(&DataKey::PausedAt)
                .unwrap_or(0);
            let elapsed = env.ledger().timestamp().saturating_sub(paused_at);
            let accrued: u64 = env
                .storage()
                .instance()
                .get(&DataKey::AccruedPauseDuration)
                .unwrap_or(0);
            env.storage()
                .instance()
                .set(&DataKey::AccruedPauseDuration, &accrued.saturating_add(elapsed));
            env.storage().instance().remove(&DataKey::PausedAt);
            zenith_common::set_paused(&env, &DataKey::Paused, false);
            events::unpaused(&env);
        }
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        ttl::extend_instance(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        if zenith_common::is_paused(&env, &DataKey::Paused) {
            let paused_at: u64 = env
                .storage()
                .instance()
                .get(&DataKey::PausedAt)
                .unwrap_or(0);
            let elapsed = env.ledger().timestamp().saturating_sub(paused_at);
            let accrued: u64 = env
                .storage()
                .instance()
                .get(&DataKey::AccruedPauseDuration)
                .unwrap_or(0);
            env.storage()
                .instance()
                .set(&DataKey::AccruedPauseDuration, &accrued.saturating_add(elapsed));
            env.storage().instance().remove(&DataKey::PausedAt);
            zenith_common::set_paused(&env, &DataKey::Paused, false);
            events::unpaused(&env);
        }
    }

    // ── admin: token allowlist (issue #193) ────────────────────────────────

    /// Adds or removes `token` from the allowlist.
    ///
    /// `allowed = true`: new deposits of this token are permitted.
    /// `allowed = false`: new deposits are blocked; existing tagged balances
    /// remain withdrawable and are never stranded.
    pub fn set_token_allowed(env: Env, token: Address, allowed: bool) {
        ttl::extend_instance(&env);
        require_admin(&env);
        set_token_allowed_inner(&env, token, allowed);
    }

    pub fn set_token_allowed_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        token: Address,
        allowed: bool,
    ) {
        ttl::extend_instance(&env);
        require_admin_via_multisig(&env, multisig_contract, action_id, ActionClass::Standard);
        set_token_allowed_inner(&env, token, allowed);
    }

    // ── admin: integrator registry (issue #193) ────────────────────────────

    /// Registers or deregisters `integrator` as a trusted integrator.
    ///
    /// `allowed = true`: the integrator may own tags and receive deposits.
    /// `allowed = false`: further deposits into its tags are rejected;
    /// existing balances remain withdrawable.
    pub fn set_integrator(env: Env, integrator: Address, allowed: bool) {
        ttl::extend_instance(&env);
        require_admin(&env);
        set_integrator_inner(&env, integrator, allowed);
    }

    pub fn set_integrator_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        integrator: Address,
        allowed: bool,
    ) {
        ttl::extend_instance(&env);
        require_admin_via_multisig(&env, multisig_contract, action_id, ActionClass::Standard);
        set_integrator_inner(&env, integrator, allowed);
    }

    // ── depositors ─────────────────────────────────────────────────────────

    /// Pulls `amount` of an allowlisted `token` from `from` and credits
    /// it to `(token, tag)`. Requires `from`'s auth and the tag owner's
    /// auth (implicit when the owner contract is the caller). Rejects
    /// non-allowlisted tokens (`TokenNotAllowed`) and tags owned by an
    /// unregistered integrator (`UnregisteredIntegrator`).
    ///
    /// Credits and returns the **measured** balance delta (fee-on-transfer
    /// tokens: credits only what actually arrived).
    pub fn deposit(env: Env, from: Address, token: Address, tag: Tag, amount: i128) -> i128 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        from.require_auth();

        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        // Allowlist gate (issue #193): token and integrator, every deposit.
        require_deposit_allowed(&env, &token, &tag.owner);
        tag.owner.require_auth();
        let tag_owner_key = DataKey::TagOwner(tag.clone());
        if !env.storage().persistent().has(&tag_owner_key) {
            ttl::set_persistent(&env, &tag_owner_key, &tag.owner);
        }

        // Measure actual received delta (handles fee-on-transfer tokens)
        let tok = token::Client::new(&env, &token);
        let before = tok.balance(&env.current_contract_address());
        tok.transfer(&from, &env.current_contract_address(), &amount);
        let after = tok.balance(&env.current_contract_address());
        let received = after.checked_sub(before).unwrap_or(0);
        if received <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        // Update bookkeeping
        let new_escrow = get_escrow(&env, &token, &tag)
            .checked_add(received)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InvalidAmount));
        set_escrow(&env, &token, &tag, new_escrow);

        let new_total = get_total_escrowed_raw(&env, &token)
            .checked_add(received)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InvalidAmount));
        set_total_escrowed(&env, &token, new_total);

        events::deposited(&env, token, tag, from, received);
        received
    }

    /// Pays `amount` of `(token, tag)` to `to`. Requires the tag owner's
    /// auth. Panics with `InsufficientEscrowBalance` if the tag is short,
    /// `InsufficientVaultBalance` if the vault's token balance can't cover
    /// it.
    pub fn withdraw(env: Env, token: Address, tag: Tag, to: Address, amount: i128) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        // Require the tag owner's auth
        let tag_owner_key = DataKey::TagOwner(tag.clone());
        let owner: Address = ttl::get_persistent(&env, &tag_owner_key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Unauthorized));
        owner.require_auth();

        let escrow = get_escrow(&env, &token, &tag);
        if escrow < amount {
            panic_with_error!(&env, Error::InsufficientEscrowBalance);
        }

        let tok = token::Client::new(&env, &token);
        let vault_balance = tok.balance(&env.current_contract_address());
        let total_escrowed = get_total_escrowed_raw(&env, &token);

        if vault_balance < total_escrowed {
            events::shortfall_detected(&env, token.clone());
        }

        if vault_balance < amount {
            panic_with_error!(&env, Error::InsufficientVaultBalance);
        }

        // Update bookkeeping before transfer
        set_escrow(&env, &token, &tag, escrow - amount);
        let new_total = get_total_escrowed_raw(&env, &token).saturating_sub(amount);
        set_total_escrowed(&env, &token, new_total);

        tok.transfer(&env.current_contract_address(), &to, &amount);
        events::withdrawn(&env, token, tag, to, amount);
    }

    /// Permissionless emergency alternative to `withdraw`: authorized by
    /// a multisig instead of the tag owner. Meant for manual recovery
    /// when the owner contract can't produce auth itself.
    pub fn withdraw_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        token: Address,
        tag: Tag,
        to: Address,
        amount: i128,
    ) {
        ttl::extend_instance(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Critical,
            Error::Unauthorized,
        );
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let escrow = get_escrow(&env, &token, &tag);
        if escrow < amount {
            panic_with_error!(&env, Error::InsufficientEscrowBalance);
        }

        let tok = token::Client::new(&env, &token);
        let vault_balance = tok.balance(&env.current_contract_address());
        if vault_balance < amount {
            panic_with_error!(&env, Error::InsufficientVaultBalance);
        }

        set_escrow(&env, &token, &tag, escrow - amount);
        let new_total = get_total_escrowed_raw(&env, &token).saturating_sub(amount);
        set_total_escrowed(&env, &token, new_total);

        tok.transfer(&env.current_contract_address(), &to, &amount);
        events::withdrawn(&env, token, tag, to, amount);
    }

    /// Recovers `token` that landed on the vault directly, bypassing
    /// `deposit` — the actual balance minus `get_total_escrowed(token)`.
    pub fn sweep_untagged(env: Env, token: Address, to: Address) {
        ttl::extend_instance(&env);
        require_admin(&env);
        let tok = token::Client::new(&env, &token);
        let balance = tok.balance(&env.current_contract_address());
        let escrowed = get_total_escrowed_raw(&env, &token);
        if balance <= escrowed {
            panic_with_error!(&env, Error::NoUntaggedFunds);
        }
        let untagged = balance - escrowed;
        tok.transfer(&env.current_contract_address(), &to, &untagged);
        events::swept_untagged(&env, token, to);
    }

    pub fn sweep_untagged_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        token: Address,
        to: Address,
    ) {
        ttl::extend_instance(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Critical,
            Error::Unauthorized,
        );
        let tok = token::Client::new(&env, &token);
        let balance = tok.balance(&env.current_contract_address());
        let escrowed = get_total_escrowed_raw(&env, &token);
        if balance <= escrowed {
            panic_with_error!(&env, Error::NoUntaggedFunds);
        }
        let untagged = balance - escrowed;
        tok.transfer(&env.current_contract_address(), &to, &untagged);
        events::swept_untagged(&env, token, to);
    }

    // ── views ──────────────────────────────────────────────────────────────

    /// Current escrowed balance for a `u64` tag id (using the default token).
    /// This is the primary view used by options_market integration tests.
    pub fn balance_of(env: Env, id: u64) -> i128 {
        ttl::extend_instance(&env);
        let token = Self::default_token(&env);
        let tag = Self::make_legacy_tag(&env, id);
        get_escrow(&env, &token, &tag)
    }

    /// Current escrowed balance for a specific `(token, tag)` pair.
    pub fn balance_of_tagged(env: Env, token: Address, tag: types::Tag) -> i128 {
        ttl::extend_instance(&env);
        get_escrow(&env, &token, &tag)
    }

    /// Legacy single-token `balance_of` for a `u64` tag id.
    pub fn balance_of_legacy(env: Env, id: u64) -> i128 {
        ttl::extend_instance(&env);
        let token = Self::default_token(&env);
        let tag = Self::make_legacy_tag(&env, id);
        get_escrow(&env, &token, &tag)
    }

    /// Total escrowed across all tags for `token`.
    pub fn get_total_escrowed(env: Env, token: Address) -> i128 {
        ttl::extend_instance(&env);
        get_total_escrowed_raw(&env, &token)
    }

    /// `max(0, TotalEscrowed(token) − vault.balance(token))`.
    pub fn get_shortfall(env: Env, token: Address) -> i128 {
        ttl::extend_instance(&env);
        let tok = token::Client::new(&env, &token);
        let balance = tok.balance(&env.current_contract_address());
        let escrowed = get_total_escrowed_raw(&env, &token);
        if escrowed > balance {
            escrowed - balance
        } else {
            0
        }
    }

    pub fn get_tag_owner(env: Env, tag: Tag) -> Option<Address> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::TagOwner(tag))
    }

    /// `true` if `token` is on the allowlist.
    pub fn is_token_allowed(env: Env, token: Address) -> bool {
        ttl::extend_instance(&env);
        is_token_allowed(&env, &token)
    }

    /// `true` if `addr` is a registered integrator.
    pub fn is_integrator(env: Env, addr: Address) -> bool {
        ttl::extend_instance(&env);
        is_integrator(&env, &addr)
    }

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        zenith_common::get_admin(&env, &DataKey::Admin)
    }

    pub fn get_token(env: Env) -> Address {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Token).unwrap()
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        zenith_common::is_paused(&env, &DataKey::Paused)
    }

    pub fn get_max_pause_duration(env: Env) -> u64 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::MaxPauseDuration)
            .unwrap_or(0)
    }

    // ── legacy wrappers (default token, u64 tags) ──────────────────────

    /// Maps a plain `u64` id to `Tag { owner: legacy_owner, kind: "legacy", id }`.
    pub fn legacy_tag(env: Env, id: u64) -> types::Tag {
        ttl::extend_instance(&env);
        Self::make_legacy_tag(&env, id)
    }

    fn make_legacy_tag(env: &Env, id: u64) -> types::Tag {
        let owner: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        types::Tag {
            owner,
            kind: soroban_sdk::symbol_short!("legacy"),
            id,
        }
    }

    fn default_token(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Token).unwrap()
    }

    pub fn deposit_legacy(env: Env, from: Address, id: u64, amount: i128) -> i128 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        from.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let token = Self::default_token(&env);
        let tag = Self::make_legacy_tag(&env, id);

        // Same allowlist gate as `deposit`. Legacy tags are owned by the
        // admin, so only the token check can fail here.
        require_deposit_allowed(&env, &token, &tag.owner);
        tag.owner.require_auth();
        let tag_owner_key = DataKey::TagOwner(tag.clone());
        if !env.storage().persistent().has(&tag_owner_key) {
            ttl::set_persistent(&env, &tag_owner_key, &tag.owner);
        }

        let tok = token::Client::new(&env, &token);
        let before = tok.balance(&env.current_contract_address());
        tok.transfer(&from, &env.current_contract_address(), &amount);
        let after = tok.balance(&env.current_contract_address());
        let received = after.checked_sub(before).unwrap_or(0);
        if received <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let new_escrow = get_escrow(&env, &token, &tag)
            .checked_add(received)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InvalidAmount));
        set_escrow(&env, &token, &tag, new_escrow);
        let new_total = get_total_escrowed_raw(&env, &token)
            .checked_add(received)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InvalidAmount));
        set_total_escrowed(&env, &token, new_total);
        received
    }

    pub fn withdraw_legacy(env: Env, id: u64, to: Address, amount: i128) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let token = Self::default_token(&env);
        let tag = Self::make_legacy_tag(&env, id);

        let tag_owner_key = DataKey::TagOwner(tag.clone());
        let owner: Address = ttl::get_persistent(&env, &tag_owner_key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Unauthorized));
        owner.require_auth();

        let escrow = get_escrow(&env, &token, &tag);
        if escrow < amount {
            panic_with_error!(&env, Error::InsufficientEscrowBalance);
        }

        let tok = token::Client::new(&env, &token);
        let vault_balance = tok.balance(&env.current_contract_address());
        if vault_balance < amount {
            panic_with_error!(&env, Error::InsufficientVaultBalance);
        }

        set_escrow(&env, &token, &tag, escrow - amount);
        let new_total = get_total_escrowed_raw(&env, &token).saturating_sub(amount);
        set_total_escrowed(&env, &token, new_total);
        tok.transfer(&env.current_contract_address(), &to, &amount);
    }
}
