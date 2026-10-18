#![no_std]

//! Zenith Protocol — Price Oracle on Stellar Soroban
//!
//! Aggregates prices from multiple authorized feeders using a median
//! with a configurable quorum (min_reports). Stale reports are excluded.

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, Symbol, Vec};

mod error;
mod events;
mod math;
mod migrations;
mod oracle_adapter;
mod outlier_filter;
mod round_history;
mod feeder_staking;
mod storage;
mod ttl;
mod types;

pub use error::Error;
pub use types::DataKey;

#[cfg(test)]
mod storage_test;
#[cfg(test)]
mod test;
#[cfg(test)]
mod test_adversaries;

use error::Error as E;
use math::MAX_FEEDERS;
use types::DataKey as DK;

#[contract]
pub struct PriceOracle;

// ─── Internal helpers ────────────────────────────────────────────────────────

fn get_admin(env: &Env) -> Address {
    zenith_common::get_admin(env, &DK::Admin)
}

fn get_max_staleness(env: &Env) -> u64 {
    env.storage().instance().get(&DK::MaxStaleness).unwrap_or(3_600u64)
}

fn get_min_reports(env: &Env) -> u32 {
    env.storage().instance().get(&DK::MinReports).unwrap_or(1u32)
}

// ─── Contract ────────────────────────────────────────────────────────────────

#[contractimpl]
impl PriceOracle {
    // ── Initialization ────────────────────────────────────────────────────────

    /// One-time setup. Panics with `AlreadyInitialized` if called twice.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DK::Admin) {
            panic_with_error!(env, E::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DK::Admin, &admin);
        env.storage().instance().set(&DK::MaxStaleness, &3_600u64);
        env.storage().instance().set(&DK::MinReports, &1u32);
        env.storage().instance().set(&DK::Paused, &false);
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
        crate::ttl::extend_instance(&env);
    }

    // ── Admin: feeder management ──────────────────────────────────────────────

    /// Authorize a new price reporter.
    pub fn add_feeder(env: Env, feeder: Address) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        if storage::is_feeder(&env, &feeder) {
            panic_with_error!(env, E::FeederAlreadyAdded);
        }
        let count = storage::feeder_list(&env).len();
        if count >= MAX_FEEDERS {
            panic_with_error!(env, E::TooManyFeeders);
        }
        storage::add_feeder(&env, &feeder);
        // Extend TTL for the feeder membership entries
        crate::ttl::extend_persistent(&env, &DK::Feeders);
        crate::ttl::extend_persistent(&env, &DK::IsFeeder(feeder.clone()));
        events::feeder_added(&env, feeder);
    }

    pub fn add_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        feeder: Address,
    ) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DK::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            E::Unauthorized,
        );
        if storage::is_feeder(&env, &feeder) {
            panic_with_error!(env, E::FeederAlreadyAdded);
        }
        let count = storage::feeder_list(&env).len();
        if count >= MAX_FEEDERS {
            panic_with_error!(env, E::TooManyFeeders);
        }
        storage::add_feeder(&env, &feeder);
        crate::ttl::extend_persistent(&env, &DK::Feeders);
        crate::ttl::extend_persistent(&env, &DK::IsFeeder(feeder.clone()));
        events::feeder_added(&env, feeder);
    }

    /// Revoke a price reporter.
    pub fn remove_feeder(env: Env, feeder: Address) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        if !storage::is_feeder(&env, &feeder) {
            panic_with_error!(env, E::FeederNotFound);
        }
        storage::remove_feeder(&env, &feeder);
        crate::ttl::extend_persistent(&env, &DK::Feeders);
        events::feeder_removed(&env, feeder);
    }

    pub fn remove_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        feeder: Address,
    ) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DK::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            E::Unauthorized,
        );
        if !storage::is_feeder(&env, &feeder) {
            panic_with_error!(env, E::FeederNotFound);
        }
        storage::remove_feeder(&env, &feeder);
        crate::ttl::extend_persistent(&env, &DK::Feeders);
        events::feeder_removed(&env, feeder);
    }

    // ── Admin: configuration ──────────────────────────────────────────────────

    pub fn set_max_staleness(env: Env, seconds: u64) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        if seconds == 0 {
            panic_with_error!(env, E::InvalidStaleness);
        }
        env.storage().instance().set(&DK::MaxStaleness, &seconds);
        events::max_staleness_updated(&env, seconds);
    }

    pub fn set_max_staleness_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        seconds: u64,
    ) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DK::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            E::Unauthorized,
        );
        if seconds == 0 {
            panic_with_error!(env, E::InvalidStaleness);
        }
        env.storage().instance().set(&DK::MaxStaleness, &seconds);
        events::max_staleness_updated(&env, seconds);
    }

    pub fn set_min_reports(env: Env, count: u32) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        if count == 0 {
            panic_with_error!(env, E::InvalidMinReports);
        }
        env.storage().instance().set(&DK::MinReports, &count);
        events::min_reports_updated(&env, count);
    }

    pub fn set_min_reports_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        count: u32,
    ) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DK::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            E::Unauthorized,
        );
        if count == 0 {
            panic_with_error!(env, E::InvalidMinReports);
        }
        env.storage().instance().set(&DK::MinReports, &count);
        events::min_reports_updated(&env, count);
    }

    // ── Admin: pause / unpause ────────────────────────────────────────────────

    pub fn pause(env: Env) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        zenith_common::set_paused(&env, &DK::Paused, true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        zenith_common::set_paused(&env, &DK::Paused, false);
        events::unpaused(&env);
    }

    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DK::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Emergency,
            E::Unauthorized,
        );
        zenith_common::set_paused(&env, &DK::Paused, true);
        events::paused(&env);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DK::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            E::Unauthorized,
        );
        zenith_common::set_paused(&env, &DK::Paused, false);
        events::unpaused(&env);
    }

    // ── Admin transfer ────────────────────────────────────────────────────────

    pub fn transfer_admin(env: Env, new_admin: Address) {
        crate::ttl::extend_instance(&env);
        let old = zenith_common::set_admin(&env, &DK::Admin, &new_admin);
        old.require_auth();
        events::admin_transferred(&env, old, new_admin);
    }

    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DK::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Critical,
            E::Unauthorized,
        );
        let old = zenith_common::set_admin(&env, &DK::Admin, &new_admin);
        events::admin_transferred(&env, old, new_admin);
    }

    // ── Feeders: price reporting ──────────────────────────────────────────────

    /// Record this feeder's price observation for `symbol`.
    pub fn report_price(env: Env, feeder: Address, symbol: Symbol, price: i128) {
        crate::ttl::extend_instance(&env);
        feeder.require_auth();
        if !storage::is_feeder(&env, &feeder) {
            panic_with_error!(env, E::NotAFeeder);
        }
        if price <= 0 {
            panic_with_error!(env, E::InvalidPrice);
        }
        if zenith_common::is_paused(&env, &DK::Paused) {
            panic_with_error!(env, E::ContractPaused);
        }
        let timestamp = env.ledger().timestamp();
        let key = DK::PriceReport(symbol.clone(), feeder.clone());
        crate::ttl::set_persistent(&env, &key, &(price, timestamp));
        events::price_reported(&env, feeder, symbol, price);
    }

    // ── Aggregation ───────────────────────────────────────────────────────────

    /// Median of fresh feeder reports, or `None` if quorum isn't met.
    ///
    /// Does NOT block while paused — a pause freezes changes to the feed,
    /// it doesn't hide the last-known price.
    pub fn get_price(env: Env, symbol: Symbol) -> Option<i128> {
        crate::ttl::extend_instance(&env);
        let now = env.ledger().timestamp();
        let max_staleness = get_max_staleness(&env);
        let min_reports = get_min_reports(&env);
        let feeders = storage::feeder_list(&env);

        let mut values = [0i128; MAX_FEEDERS as usize];
        let mut count = 0usize;

        for feeder in feeders.iter() {
            // Only count authorized feeders
            if !storage::is_feeder(&env, &feeder) {
                continue;
            }
            let key = DK::PriceReport(symbol.clone(), feeder.clone());
            let report: Option<(i128, u64)> = crate::ttl::get_persistent(&env, &key);
            if let Some((price, timestamp)) = report {
                if now.saturating_sub(timestamp) <= max_staleness {
                    if count < MAX_FEEDERS as usize {
                        values[count] = price;
                        count += 1;
                    }
                }
            }
        }

        if count == 0 || (count as u32) < min_reports {
            return None;
        }

        Some(math::median(&mut values, count))
    }

    /// One feeder's raw report (regardless of freshness or current auth status).
    pub fn get_latest_report(env: Env, symbol: Symbol, feeder: Address) -> Option<(i128, u64)> {
        crate::ttl::extend_instance(&env);
        crate::ttl::get_persistent(&env, &DK::PriceReport(symbol, feeder))
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn is_feeder(env: Env, feeder: Address) -> bool {
        crate::ttl::extend_instance(&env);
        storage::is_feeder(&env, &feeder)
    }

    pub fn get_feeder_count(env: Env) -> u32 {
        crate::ttl::extend_instance(&env);
        storage::feeder_list(&env).len()
    }

    pub fn get_max_staleness(env: Env) -> u64 {
        crate::ttl::extend_instance(&env);
        get_max_staleness(&env)
    }

    pub fn get_min_reports(env: Env) -> u32 {
        crate::ttl::extend_instance(&env);
        get_min_reports(&env)
    }

    pub fn get_admin(env: Env) -> Address {
        crate::ttl::extend_instance(&env);
        get_admin(&env)
    }

    pub fn is_paused(env: Env) -> bool {
        crate::ttl::extend_instance(&env);
        zenith_common::is_paused(&env, &DK::Paused)
    }
}
