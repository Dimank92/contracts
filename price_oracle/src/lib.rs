#![no_std]

//! Zenith Price Oracle — on-chain price feed for the options_market contract.
//!
//! A small set of admin-authorized feeders report prices per symbol. The
//! aggregate callers read is computed by `outlier_filter::aggregate`:
//!
//! 1. **freshness** — only reports newer than `max_staleness` seconds
//!    count (admin-settable, applied before anything else);
//! 2. **outlier filter** — reports too far from the median (MAD band,
//!    see `outlier_filter`) are discarded;
//! 3. **median** of what's left;
//! 4. **quorum** — refused unless at least `min_reports` fresh reports
//!    survived the filter, so a feeder set that has mostly gone stale
//!    can't silently degrade to a single reporter.
//!
//! `get_price` runs that pipeline as a read (`None` when it fails) and is
//! what options_market cross-calls. `aggregate` runs it permissionlessly
//! and records the result as a `PriceRound`, failing with a typed error
//! (and writing nothing) when there are no fresh reports or no quorum.
//!
//! Every admin function is a thin entrypoint over an `x_inner(env, Auth,
//! ...)` that starts with `zenith_common::require_admin_or_multisig`; the
//! `_via_multisig` twins only differ in the `Auth` they pass.

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, BytesN, Env, Symbol, Vec};
use zenith_common::{self as common, ActionClass, Auth};

#[cfg(test)]
mod storage_test;
#[cfg(test)]
mod test;
#[cfg(test)]
mod test_adversaries;

mod error;
mod events;
pub mod math;
mod migrations;
#[allow(dead_code)]
mod oracle_adapter;
pub mod outlier_filter;
mod storage;
mod ttl;
mod types;

use error::Error;
use math::MAX_FEEDERS;
use outlier_filter::{aggregate, AggregateError, FilterConfig, MAX_REPORTS};
pub use types::{DataKey, PriceRound};

const DEFAULT_MAX_STALENESS: u64 = 3600; // 1 hour
const DEFAULT_MIN_REPORTS: u32 = 1; // any single fresh report is enough, the original behavior

fn gate(env: &Env, auth: &Auth, class: ActionClass) {
    common::require_admin_or_multisig(env, &DataKey::Admin, auth, class, Error::Unauthorized);
}

fn require_not_paused(env: &Env) {
    common::require_not_paused(env, &DataKey::Paused, Error::ContractPaused);
}

/// Copies every feeder's report for `symbol` that is inside the
/// freshness window into `buffer`; returns how many.
fn fresh_reports(env: &Env, symbol: &Symbol, buffer: &mut [i128; MAX_REPORTS]) -> usize {
    let max_staleness: u64 = env
        .storage()
        .instance()
        .get(&DataKey::MaxStaleness)
        .unwrap_or(DEFAULT_MAX_STALENESS);
    let now = env.ledger().timestamp();
    let mut count = 0usize;
    for feeder in storage::feeder_list(env).iter() {
        if count == MAX_REPORTS {
            break;
        }
        let report: Option<(i128, u64)> =
            ttl::get_persistent(env, &DataKey::PriceReport(symbol.clone(), feeder));
        if let Some((price, reported_at)) = report {
            // A report stamped in the future can't happen through
            // report_price; treat it as stale rather than trusting it.
            if now
                .checked_sub(reported_at)
                .is_some_and(|age| age <= max_staleness)
            {
                buffer[count] = price;
                count += 1;
            }
        }
    }
    count
}

fn min_reports(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::MinReports)
        .unwrap_or(DEFAULT_MIN_REPORTS)
}

#[contract]
pub struct PriceOracle;

#[contractimpl]
impl PriceOracle {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    pub fn initialize(env: Env, admin: Address) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &DEFAULT_MAX_STALENESS);
        env.storage()
            .instance()
            .set(&DataKey::MinReports, &DEFAULT_MIN_REPORTS);
        common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    // ── Admin: configuration ──────────────────────────────────────────────────

    /// Sets the freshness window (seconds). Reports older than this are
    /// ignored before outlier filtering.
    pub fn set_max_staleness(env: Env, seconds: u64) {
        Self::set_max_staleness_inner(&env, Auth::Admin, seconds);
    }

    pub fn set_max_staleness_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        seconds: u64,
    ) {
        Self::set_max_staleness_inner(&env, Auth::Multisig(multisig_contract, action_id), seconds);
    }

    pub fn get_max_staleness(env: Env) -> u64 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::MaxStaleness)
            .unwrap_or(DEFAULT_MAX_STALENESS)
    }

    /// Sets how many fresh reports must survive outlier filtering for an
    /// aggregate to exist at all.
    pub fn set_min_reports(env: Env, count: u32) {
        Self::set_min_reports_inner(&env, Auth::Admin, count);
    }

    pub fn set_min_reports_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        count: u32,
    ) {
        Self::set_min_reports_inner(&env, Auth::Multisig(multisig_contract, action_id), count);
    }

    pub fn get_min_reports(env: Env) -> u32 {
        ttl::extend_instance(&env);
        min_reports(&env)
    }

    // ── Admin: transfer (two-step) ────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        common::get_admin(&env, &DataKey::Admin)
    }

    /// Step one of an admin transfer: records `new_admin` as pending.
    /// Control only moves once `new_admin` calls `accept_admin`, so a
    /// wrong or unusable address can't strand the contract. Proposing the
    /// current admin cancels a pending transfer.
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

    /// Emergency stop: blocks feeder changes, `report_price` and
    /// `aggregate`. `get_price` keeps working so settlement can still read
    /// the last fresh reports.
    pub fn pause(env: Env) {
        Self::set_paused_inner(&env, Auth::Admin, true);
    }

    pub fn unpause(env: Env) {
        Self::set_paused_inner(&env, Auth::Admin, false);
    }

    /// `Emergency` class: the short path that stays available when the
    /// admin itself is a timelock.
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

    // ── Admin: feeders ────────────────────────────────────────────────────────

    pub fn add_feeder(env: Env, feeder: Address) {
        Self::add_feeder_inner(&env, Auth::Admin, feeder);
    }

    pub fn add_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        feeder: Address,
    ) {
        Self::add_feeder_inner(&env, Auth::Multisig(multisig_contract, action_id), feeder);
    }

    pub fn remove_feeder(env: Env, feeder: Address) {
        Self::remove_feeder_inner(&env, Auth::Admin, feeder);
    }

    pub fn remove_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        feeder: Address,
    ) {
        Self::remove_feeder_inner(&env, Auth::Multisig(multisig_contract, action_id), feeder);
    }

    pub fn is_feeder(env: Env, address: Address) -> bool {
        ttl::extend_instance(&env);
        storage::is_feeder(&env, &address)
    }

    pub fn get_feeder_count(env: Env) -> u32 {
        ttl::extend_instance(&env);
        storage::feeder_list(&env).len()
    }

    pub fn get_feeders(env: Env) -> Vec<Address> {
        ttl::extend_instance(&env);
        storage::feeder_list(&env)
    }

    // ── Feeders ───────────────────────────────────────────────────────────────

    pub fn report_price(env: Env, feeder: Address, symbol: Symbol, price: i128) {
        ttl::extend_instance(&env);
        feeder.require_auth();
        require_not_paused(&env);
        migrations::require_current(&env);
        if !storage::is_feeder(&env, &feeder) {
            panic_with_error!(&env, Error::NotAFeeder);
        }
        if price <= 0 {
            panic_with_error!(&env, Error::InvalidPrice);
        }

        let now = env.ledger().timestamp();
        ttl::set_persistent(
            &env,
            &DataKey::PriceReport(symbol.clone(), feeder.clone()),
            &(price, now),
        );
        events::price_reported(&env, feeder, symbol, price);
    }

    pub fn get_latest_report(env: Env, symbol: Symbol, feeder: Address) -> Option<(i128, u64)> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::PriceReport(symbol, feeder))
    }

    // ── Aggregation ───────────────────────────────────────────────────────────

    /// Freshness → outlier filter → median → quorum over the current
    /// reports. `None` when no report is fresh or fewer than
    /// `min_reports` survive the filter. Never panics on report data.
    pub fn get_price(env: Env, symbol: Symbol) -> Option<i128> {
        ttl::extend_instance(&env);
        let mut buffer = [0i128; MAX_REPORTS];
        let count = fresh_reports(&env, &symbol, &mut buffer);
        aggregate(&buffer[..count], min_reports(&env), FilterConfig::default())
            .ok()
            .map(|a| a.price)
    }

    /// Runs the same pipeline as `get_price` and records the result as
    /// the symbol's next `PriceRound`. Permissionless (a keeper or any
    /// consumer can pay for it). Fails with `NoFreshReports` or
    /// `QuorumNotMet` — writing nothing — instead of recording a price
    /// the quorum doesn't back.
    pub fn aggregate(env: Env, symbol: Symbol) -> PriceRound {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        migrations::require_current(&env);

        let mut buffer = [0i128; MAX_REPORTS];
        let count = fresh_reports(&env, &symbol, &mut buffer);
        let agg = aggregate(&buffer[..count], min_reports(&env), FilterConfig::default())
            .unwrap_or_else(|e| match e {
                AggregateError::NoReports => panic_with_error!(&env, Error::NoFreshReports),
                AggregateError::QuorumNotMet { .. } => {
                    panic_with_error!(&env, Error::QuorumNotMet)
                }
                AggregateError::TooManyReports => panic_with_error!(&env, Error::TooManyFeeders),
            });

        let key = DataKey::AggregatedPrice(symbol.clone());
        let previous: Option<PriceRound> = ttl::get_persistent(&env, &key);
        let round = PriceRound {
            round_id: previous.map_or(1, |r| r.round_id.saturating_add(1)),
            price: agg.price,
            timestamp: env.ledger().timestamp(),
            reports_used: agg.used,
            reports_discarded: agg.discarded,
        };
        ttl::set_persistent(&env, &key, &round);
        events::round_aggregated(&env, symbol, &round);
        round
    }

    /// The last round `aggregate` recorded for `symbol`, if any.
    pub fn get_latest_round(env: Env, symbol: Symbol) -> Option<PriceRound> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::AggregatedPrice(symbol))
    }
}

/// Admin bodies shared by each entrypoint and its `_via_multisig` twin.
/// Auth is resolved first, before any other state is read.
impl PriceOracle {
    fn set_max_staleness_inner(env: &Env, auth: Auth, seconds: u64) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Standard);
        if seconds == 0 {
            panic_with_error!(env, Error::InvalidStaleness);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &seconds);
        events::max_staleness_updated(env, seconds);
    }

    fn set_min_reports_inner(env: &Env, auth: Auth, count: u32) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Standard);
        if count == 0 || count > MAX_FEEDERS {
            panic_with_error!(env, Error::InvalidMinReports);
        }
        env.storage().instance().set(&DataKey::MinReports, &count);
        events::min_reports_updated(env, count);
    }

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

    fn add_feeder_inner(env: &Env, auth: Auth, feeder: Address) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Standard);
        require_not_paused(env);
        migrations::require_current(env);
        if storage::is_feeder(env, &feeder) {
            panic_with_error!(env, Error::FeederAlreadyAdded);
        }
        if storage::feeder_list(env).len() >= MAX_FEEDERS {
            panic_with_error!(env, Error::TooManyFeeders);
        }
        storage::add_feeder(env, &feeder);
        events::feeder_added(env, feeder);
    }

    fn remove_feeder_inner(env: &Env, auth: Auth, feeder: Address) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Standard);
        require_not_paused(env);
        migrations::require_current(env);
        if !storage::is_feeder(env, &feeder) {
            panic_with_error!(env, Error::FeederNotFound);
        }
        storage::remove_feeder(env, &feeder);
        events::feeder_removed(env, feeder);
    }
}
