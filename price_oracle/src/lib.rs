#![no_std]

//! Zenith Protocol — Price Oracle
//!
//! A small set of admin-authorized feeders report per-symbol prices.
//! The on-chain aggregate is a median across all currently-fresh reports,
//! gated by `min_reports` so a single fresh feeder can't unilaterally
//! determine the price.
//!
//! Issue #194 adds a **symbol registry**: the admin must register each
//! symbol (e.g. "XLM", "BTC", "ETH", "SOL") with per-symbol metadata
//! (freshness-window override and decimal precision) before any feeder
//! can report a price for it. `report_price` rejects unregistered
//! symbols with `SymbolNotRegistered`.
//!
//! Symbols are canonical: 1–12 characters of `A-Z`, `0-9` or `_`, so
//! `"xlm"` can never become a second registry entry next to `"XLM"`
//! (Soroban symbols cannot contain whitespace at all). The registry is
//! capped at `MAX_SYMBOLS` entries.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, Address, Env, Symbol, SymbolStr, TryFromVal, Vec,
};

mod error;
mod events;
mod math;
mod storage;

pub mod types;

#[cfg(test)]
mod test;

use error::Error;
use math::{median, MAX_FEEDERS};
use storage::{add_feeder, feeder_list, is_feeder, remove_feeder};
use types::{DataKey, SymbolMeta};
use zenith_common::{require_admin_or_multisig, ActionClass, Auth};

mod migrations;
mod ttl;

/// Maximum number of symbols that can be registered at once.
/// Keeps iteration over per-symbol state bounded.
pub const MAX_SYMBOLS: u32 = 64;

/// Longest accepted symbol, in characters.
pub const MAX_SYMBOL_LEN: usize = 12;

/// Upper bound on `SymbolMeta::decimals`.
pub const MAX_DECIMALS: u32 = 18;

// ─── helpers ─────────────────────────────────────────────────────────────────

fn require_admin(env: &Env) {
    zenith_common::get_admin(env, &DataKey::Admin).require_auth();
}

fn require_not_paused(env: &Env) {
    zenith_common::require_not_paused(env, &DataKey::Paused, Error::ContractPaused);
}

fn get_max_staleness(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::MaxStaleness)
        .unwrap_or(3_600u64) // 1 hour default
}

fn get_min_reports(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::MinReports)
        .unwrap_or(1u32)
}

/// Returns the effective max-staleness for `symbol`: the per-symbol
/// override if set, otherwise the contract-wide value.
fn effective_staleness(env: &Env, symbol: &Symbol) -> u64 {
    if let Some(meta) = ttl::get_persistent::<_, SymbolMeta>(env, &DataKey::SymbolMeta(symbol.clone())) {
        meta.max_staleness_override.unwrap_or_else(|| get_max_staleness(env))
    } else {
        get_max_staleness(env)
    }
}

fn symbol_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::SymbolCount)
        .unwrap_or(0u32)
}

/// Rejects anything but a canonical ticker: 1–`MAX_SYMBOL_LEN`
/// characters, each `A-Z`, `0-9` or `_`. Lowercase is rejected rather
/// than folded so that the symbol a feeder reports is byte-identical to
/// the registered one.
fn require_canonical_symbol(env: &Env, symbol: &Symbol) {
    let canonical = match SymbolStr::try_from_val(env, &symbol.to_symbol_val()) {
        Ok(s) => {
            let s: &str = s.as_ref();
            !s.is_empty()
                && s.len() <= MAX_SYMBOL_LEN
                && s
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        }
        Err(_) => false,
    };
    if !canonical {
        panic_with_error!(env, Error::InvalidSymbol);
    }
}

fn register_symbol_inner(env: &Env, symbol: Symbol, meta: SymbolMeta) {
    require_canonical_symbol(env, &symbol);
    if meta.max_staleness_override == Some(0) || meta.decimals > MAX_DECIMALS {
        panic_with_error!(env, Error::InvalidSymbolMeta);
    }
    let key = DataKey::SymbolMeta(symbol.clone());
    if env.storage().persistent().has(&key) {
        panic_with_error!(env, Error::SymbolAlreadyRegistered);
    }
    let count = symbol_count(env);
    if count >= MAX_SYMBOLS {
        panic_with_error!(env, Error::TooManySymbols);
    }
    ttl::set_persistent(env, &key, &meta);
    env.storage()
        .instance()
        .set(&DataKey::SymbolCount, &(count + 1));
    events::symbol_registered(env, symbol, meta);
}

/// Drops the registry entry and the aggregated-price cache. Individual
/// feeder reports are kept for audit (`get_latest_report`), but
/// `get_price` returns `None` for an unregistered symbol and
/// `report_price` rejects it, so nothing downstream can consume them.
/// If the symbol is later re-registered, reports still inside the
/// freshness window count again — the same feeders signed them for the
/// same symbol.
fn remove_symbol_inner(env: &Env, symbol: Symbol) {
    let key = DataKey::SymbolMeta(symbol.clone());
    if !env.storage().persistent().has(&key) {
        panic_with_error!(env, Error::SymbolNotRegistered);
    }
    env.storage().persistent().remove(&key);
    env.storage()
        .persistent()
        .remove(&DataKey::AggregatedPrice(symbol.clone()));
    env.storage()
        .instance()
        .set(&DataKey::SymbolCount, &symbol_count(env).saturating_sub(1));
    events::symbol_removed(env, symbol);
}

// ─── contract ────────────────────────────────────────────────────────────────

#[contract]
pub struct PriceOracle;

#[contractimpl]
impl PriceOracle {
    // ── lifecycle ──────────────────────────────────────────────────────────

    /// One-time setup. Defaults `max_staleness` to 1 hour and
    /// `min_reports` to 1 (the original single-feeder-is-enough behaviour).
    pub fn initialize(env: Env, admin: Address) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &3_600u64);
        env.storage()
            .instance()
            .set(&DataKey::MinReports, &1u32);
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    // ── keeper ─────────────────────────────────────────────────────────────

    /// Permissionless keeper: extends instance + any named persistent
    /// entries that exist.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    // ── admin: transfer / pause ────────────────────────────────────────────

    /// Hands off control to `new_admin`. Requires the **current** admin's
    /// signature.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        ttl::extend_instance(&env);
        require_admin(&env);
        let old = zenith_common::set_admin(&env, &DataKey::Admin, &new_admin);
        events::admin_transferred(&env, old, new_admin);
    }

    /// Permissionless alternative to `transfer_admin`: checks
    /// `is_executable(action_id, Critical)` on the multisig instead of the
    /// admin's own signature.
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

    /// Emergency stop. Blocks `add_feeder`, `remove_feeder`,
    /// `report_price`. Does **not** block `get_price`.
    pub fn pause(env: Env) {
        ttl::extend_instance(&env);
        require_admin(&env);
        zenith_common::set_paused(&env, &DataKey::Paused, true);
        events::paused(&env);
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
        zenith_common::set_paused(&env, &DataKey::Paused, true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        ttl::extend_instance(&env);
        require_admin(&env);
        zenith_common::set_paused(&env, &DataKey::Paused, false);
        events::unpaused(&env);
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
        zenith_common::set_paused(&env, &DataKey::Paused, false);
        events::unpaused(&env);
    }

    // ── admin: staleness / quorum ──────────────────────────────────────────

    /// Sets the contract-wide max-staleness bound (seconds). Rejects zero.
    pub fn set_max_staleness(env: Env, seconds: u64) {
        ttl::extend_instance(&env);
        require_admin(&env);
        if seconds == 0 {
            panic_with_error!(&env, Error::InvalidStaleness);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &seconds);
        events::max_staleness_updated(&env, seconds);
    }

    pub fn set_max_staleness_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        seconds: u64,
    ) {
        ttl::extend_instance(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        if seconds == 0 {
            panic_with_error!(&env, Error::InvalidStaleness);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &seconds);
        events::max_staleness_updated(&env, seconds);
    }

    /// Sets the minimum number of fresh reports before `get_price` returns
    /// an aggregate. Rejects zero.
    pub fn set_min_reports(env: Env, count: u32) {
        ttl::extend_instance(&env);
        require_admin(&env);
        if count == 0 {
            panic_with_error!(&env, Error::InvalidMinReports);
        }
        env.storage().instance().set(&DataKey::MinReports, &count);
        events::min_reports_updated(&env, count);
    }

    pub fn set_min_reports_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        count: u32,
    ) {
        ttl::extend_instance(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        if count == 0 {
            panic_with_error!(&env, Error::InvalidMinReports);
        }
        env.storage().instance().set(&DataKey::MinReports, &count);
        events::min_reports_updated(&env, count);
    }

    // ── admin: feeders ─────────────────────────────────────────────────────

    /// Authorizes `feeder` to submit price reports. Capped at `MAX_FEEDERS`.
    pub fn add_feeder(env: Env, feeder: Address) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        require_admin(&env);
        if is_feeder(&env, &feeder) {
            panic_with_error!(&env, Error::FeederAlreadyAdded);
        }
        if feeder_list(&env).len() >= MAX_FEEDERS {
            panic_with_error!(&env, Error::TooManyFeeders);
        }
        add_feeder(&env, &feeder);
        events::feeder_added(&env, feeder);
    }

    pub fn add_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        feeder: Address,
    ) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        if is_feeder(&env, &feeder) {
            panic_with_error!(&env, Error::FeederAlreadyAdded);
        }
        if feeder_list(&env).len() >= MAX_FEEDERS {
            panic_with_error!(&env, Error::TooManyFeeders);
        }
        add_feeder(&env, &feeder);
        events::feeder_added(&env, feeder);
    }

    /// Revokes a feeder's authorization. Past reports remain readable via
    /// `get_latest_report` but no longer count toward `get_price`.
    pub fn remove_feeder(env: Env, feeder: Address) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        require_admin(&env);
        if !is_feeder(&env, &feeder) {
            panic_with_error!(&env, Error::FeederNotFound);
        }
        remove_feeder(&env, &feeder);
        events::feeder_removed(&env, feeder);
    }

    pub fn remove_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        feeder: Address,
    ) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        if !is_feeder(&env, &feeder) {
            panic_with_error!(&env, Error::FeederNotFound);
        }
        remove_feeder(&env, &feeder);
        events::feeder_removed(&env, feeder);
    }

    // ── admin: symbol registry (#194) ──────────────────────────────────────

    /// Registers `symbol` with `meta`, allowing feeders to report prices
    /// for it and integrators to call `get_price` on it.
    ///
    /// Fails with `InvalidSymbol` unless `symbol` is canonical (see the
    /// crate docs), `InvalidSymbolMeta` for a zero staleness override or
    /// `decimals > MAX_DECIMALS`, `SymbolAlreadyRegistered` if it is
    /// already registered (use `remove_symbol` first to change metadata),
    /// and `TooManySymbols` past `MAX_SYMBOLS`. Admin only (or multisig
    /// via Standard class).
    pub fn register_symbol(env: Env, symbol: Symbol, meta: SymbolMeta) {
        ttl::extend_instance(&env);
        require_admin(&env);
        register_symbol_inner(&env, symbol, meta);
    }

    pub fn register_symbol_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        symbol: Symbol,
        meta: SymbolMeta,
    ) {
        ttl::extend_instance(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        register_symbol_inner(&env, symbol, meta);
    }

    /// Removes `symbol` from the registry. After removal:
    /// - `report_price` rejects new reports for this symbol.
    /// - `get_price` returns `None` immediately, whatever reports exist.
    /// - `get_latest_report` still works — historical data is preserved for
    ///   audit purposes.
    ///
    /// The `AggregatedPrice` cache for the symbol is also cleared to
    /// prevent stale cached values from being served. Reports still fresh
    /// when a symbol is re-registered count toward `get_price` again.
    pub fn remove_symbol(env: Env, symbol: Symbol) {
        ttl::extend_instance(&env);
        require_admin(&env);
        remove_symbol_inner(&env, symbol);
    }

    pub fn remove_symbol_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        symbol: Symbol,
    ) {
        ttl::extend_instance(&env);
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        remove_symbol_inner(&env, symbol);
    }

    // ── feeders ────────────────────────────────────────────────────────────

    /// Records this feeder's own price observation for `symbol`.
    ///
    /// Rejects:
    /// - callers that are not in the authorized feeder list (`NotAFeeder`)
    /// - symbols not in the registry (`SymbolNotRegistered`) — issue #194
    /// - non-positive prices (`InvalidPrice`)
    /// - calls while paused (`ContractPaused`)
    pub fn report_price(env: Env, feeder: Address, symbol: Symbol, price: i128) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        feeder.require_auth();
        if !is_feeder(&env, &feeder) {
            panic_with_error!(&env, Error::NotAFeeder);
        }
        // Symbol registry gate (issue #194)
        if !env
            .storage()
            .persistent()
            .has(&DataKey::SymbolMeta(symbol.clone()))
        {
            panic_with_error!(&env, Error::SymbolNotRegistered);
        }
        if price <= 0 {
            panic_with_error!(&env, Error::InvalidPrice);
        }
        let report_key = DataKey::PriceReport(symbol.clone(), feeder.clone());
        let now = env.ledger().timestamp();
        ttl::set_persistent(&env, &report_key, &(price, now));
        events::price_reported(&env, feeder, symbol, price);
    }

    // ── views ──────────────────────────────────────────────────────────────

    /// Median of every currently-authorized feeder's fresh report for
    /// `symbol`, provided at least `min_reports` are fresh. Returns `None`
    /// if the symbol is not registered, no reports exist, or quorum is not
    /// met. Callers must never treat `None` as "price is zero."
    pub fn get_price(env: Env, symbol: Symbol) -> Option<i128> {
        ttl::extend_instance(&env);
        // Not registered → no price
        if !env
            .storage()
            .persistent()
            .has(&DataKey::SymbolMeta(symbol.clone()))
        {
            return None;
        }
        let staleness = effective_staleness(&env, &symbol);
        let now = env.ledger().timestamp();
        let feeders = feeder_list(&env);
        let mut values = [0i128; MAX_FEEDERS as usize];
        let mut count = 0usize;
        for f in feeders.iter() {
            if let Some((price, ts)) =
                ttl::get_persistent::<_, (i128, u64)>(&env, &DataKey::PriceReport(symbol.clone(), f))
            {
                if now.saturating_sub(ts) <= staleness {
                    values[count] = price;
                    count += 1;
                }
            }
        }
        if count < get_min_reports(&env) as usize {
            return None;
        }
        Some(median(&mut values, count))
    }

    /// One feeder's raw (price, timestamp) report regardless of freshness
    /// or current authorization — for audit purposes.
    pub fn get_latest_report(
        env: Env,
        symbol: Symbol,
        feeder: Address,
    ) -> Option<(i128, u64)> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::PriceReport(symbol, feeder))
    }

    /// Returns `true` if `symbol` is registered in the oracle registry.
    /// Integrators (e.g. `options_market`) should call this before creating
    /// a series to confirm the underlying has price support.
    pub fn is_supported(env: Env, symbol: Symbol) -> bool {
        ttl::extend_instance(&env);
        env.storage()
            .persistent()
            .has(&DataKey::SymbolMeta(symbol))
    }

    /// Returns the per-symbol metadata, or `None` if the symbol is not
    /// registered.
    pub fn get_symbol_meta(env: Env, symbol: Symbol) -> Option<SymbolMeta> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::SymbolMeta(symbol))
    }

    /// Number of registered symbols (at most `MAX_SYMBOLS`).
    pub fn get_symbol_count(env: Env) -> u32 {
        ttl::extend_instance(&env);
        symbol_count(&env)
    }

    pub fn is_feeder(env: Env, feeder: Address) -> bool {
        ttl::extend_instance(&env);
        is_feeder(&env, &feeder)
    }

    pub fn get_feeder_count(env: Env) -> u32 {
        ttl::extend_instance(&env);
        feeder_list(&env).len()
    }

    pub fn get_max_staleness(env: Env) -> u64 {
        ttl::extend_instance(&env);
        get_max_staleness(&env)
    }

    pub fn get_min_reports(env: Env) -> u32 {
        ttl::extend_instance(&env);
        get_min_reports(&env)
    }

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        zenith_common::get_admin(&env, &DataKey::Admin)
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        zenith_common::is_paused(&env, &DataKey::Paused)
    }
}
