#![no_std]

//! Zenith Price Oracle — median aggregation of authorized feeders' reports.
//!
//! `get_price` is the instantaneous aggregate: the median of the fresh
//! reports of active, undisputed feeders, once at least `min_reports` are
//! fresh. Settlement uses it unchanged.
//!
//! **History.** Each `report_price` that yields an aggregate records a
//! round with a cumulative `price * elapsed_ledgers` accumulator, so
//! `twap` is one subtraction over two observations and `get_round` /
//! `latest_round` read retained rounds (`round_history`).
//!
//! **Disputes.** A slash proposed against a feeder's bond does not
//! finalize: it holds the amount in a `Disputed` state for
//! `dispute_window_ledgers`, during which the feeder can post an evidence
//! hash and cannot report. Afterwards the admin or multisig resolves it
//! to slash or restore; if nobody does, anyone can finalize it (restore)
//! after a further grace period (`feeder_staking`).

mod error;
mod events;
mod feeder_staking;
mod math;
mod round_history;
mod storage;
mod ttl;
mod types;

#[cfg(test)]
mod storage_test;
#[cfg(test)]
mod test_features;

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Bytes, Env, Symbol};
use zenith_common::{require_admin_or_multisig, ActionClass, Auth};

pub use error::Error;
pub use types::{
    Bond, BondStatus, DataKey, Dispute, Resolution, RoundData, SlashDestination, TwapData,
};

#[contract]
pub struct PriceOracle;

#[contractimpl]
impl PriceOracle {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        ttl::extend_instance(&env);
        let s = env.storage().instance();
        s.set(&DataKey::Admin, &admin);
        s.set(&DataKey::MaxStaleness, &3600u64);
        s.set(&DataKey::MinReports, &1u32);
        s.set(
            &DataKey::DisputeWindow,
            &feeder_staking::DEFAULT_DISPUTE_WINDOW,
        );
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Address {
        zenith_common::get_admin(&env, &DataKey::Admin)
    }

    pub fn pause(env: Env) {
        admin(&env, &Auth::Admin, ActionClass::Emergency);
        zenith_common::set_paused(&env, &DataKey::Paused, true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        admin(&env, &Auth::Admin, ActionClass::Standard);
        zenith_common::set_paused(&env, &DataKey::Paused, false);
        events::unpaused(&env);
    }

    pub fn is_paused(env: Env) -> bool {
        zenith_common::is_paused(&env, &DataKey::Paused)
    }

    pub fn set_max_staleness(env: Env, seconds: u64) {
        admin(&env, &Auth::Admin, ActionClass::Standard);
        if seconds == 0 {
            panic_with_error!(&env, Error::InvalidStaleness);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &seconds);
        events::max_staleness_updated(&env, seconds);
    }

    pub fn get_max_staleness(env: Env) -> u64 {
        max_staleness(&env)
    }

    pub fn set_min_reports(env: Env, min_reports: u32) {
        admin(&env, &Auth::Admin, ActionClass::Standard);
        if min_reports == 0 {
            panic_with_error!(&env, Error::InvalidMinReports);
        }
        env.storage()
            .instance()
            .set(&DataKey::MinReports, &min_reports);
        events::min_reports_updated(&env, min_reports);
    }

    pub fn get_min_reports(env: Env) -> u32 {
        min_reports(&env)
    }

    // ── Feeders ───────────────────────────────────────────────────────────────

    pub fn add_feeder(env: Env, feeder: Address) {
        admin(&env, &Auth::Admin, ActionClass::Standard);
        zenith_common::require_not_paused(&env, &DataKey::Paused, Error::ContractPaused);
        if storage::is_feeder(&env, &feeder) {
            panic_with_error!(&env, Error::FeederAlreadyAdded);
        }
        if storage::feeder_list(&env).len() >= math::MAX_FEEDERS {
            panic_with_error!(&env, Error::TooManyFeeders);
        }
        storage::add_feeder(&env, &feeder);
        feeder_staking::on_feeder_added(&env, &feeder);
        events::feeder_added(&env, feeder);
    }

    pub fn remove_feeder(env: Env, feeder: Address) {
        admin(&env, &Auth::Admin, ActionClass::Standard);
        zenith_common::require_not_paused(&env, &DataKey::Paused, Error::ContractPaused);
        if !storage::is_feeder(&env, &feeder) {
            panic_with_error!(&env, Error::FeederNotFound);
        }
        storage::remove_feeder(&env, &feeder);
        feeder_staking::on_feeder_removed(&env, &feeder);
        events::feeder_removed(&env, feeder);
    }

    pub fn is_feeder(env: Env, feeder: Address) -> bool {
        storage::is_feeder(&env, &feeder)
    }

    pub fn get_feeder_count(env: Env) -> u32 {
        storage::feeder_list(&env).len()
    }

    // ── Reporting and aggregation ─────────────────────────────────────────────

    pub fn report_price(env: Env, feeder: Address, symbol: Symbol, price: i128) {
        ttl::extend_instance(&env);
        feeder.require_auth();
        zenith_common::require_not_paused(&env, &DataKey::Paused, Error::ContractPaused);
        if !storage::is_feeder(&env, &feeder) {
            panic_with_error!(&env, Error::NotAFeeder);
        }
        if price <= 0 || price > round_history::MAX_PRICE {
            panic_with_error!(&env, Error::InvalidPrice);
        }
        // A disputed feeder cannot keep reporting while under review.
        if feeder_staking::disputed_count(&env) > 0 && feeder_staking::is_disputed(&env, &feeder) {
            panic_with_error!(&env, Error::FeederDisputed);
        }
        ttl::set_persistent(
            &env,
            &DataKey::PriceReport(symbol.clone(), feeder.clone()),
            &(price, env.ledger().timestamp()),
        );
        events::price_reported(&env, feeder, symbol.clone(), price);
        if let Some(aggregate) = aggregate(&env, &symbol) {
            round_history::record(&env, &symbol, aggregate);
        }
    }

    pub fn get_latest_report(env: Env, symbol: Symbol, feeder: Address) -> Option<(i128, u64)> {
        env.storage()
            .persistent()
            .get(&DataKey::PriceReport(symbol, feeder))
    }

    /// Median of fresh reports from active, undisputed feeders; `None`
    /// below the `min_reports` quorum. Stays live while paused.
    pub fn get_price(env: Env, symbol: Symbol) -> Option<i128> {
        aggregate(&env, &symbol)
    }

    // ── History ───────────────────────────────────────────────────────────────

    /// Time-weighted average price over the last `window_ledgers`
    /// (clamped to 30 days). `None` if the window reaches beyond retained
    /// history, is zero, or the symbol has no rounds.
    pub fn twap(env: Env, symbol: Symbol, window_ledgers: u32) -> Option<i128> {
        round_history::twap(&env, &symbol, window_ledgers).map(|t| t.twap)
    }

    /// As `twap`, also reporting the effective window and how much of it
    /// was carried forward at the latest round's price.
    pub fn twap_data(env: Env, symbol: Symbol, window_ledgers: u32) -> Option<TwapData> {
        round_history::twap(&env, &symbol, window_ledgers)
    }

    pub fn get_round(env: Env, symbol: Symbol, round_id: u64) -> Option<RoundData> {
        round_history::get(&env, &symbol, round_id)
    }

    pub fn latest_round(env: Env, symbol: Symbol) -> Option<RoundData> {
        round_history::latest(&env, &symbol)
    }

    // ── Bonds and disputes ────────────────────────────────────────────────────

    pub fn deposit_bond(env: Env, feeder: Address, amount: i128) {
        ttl::extend_instance(&env);
        feeder.require_auth();
        feeder_staking::deposit(&env, &feeder, amount);
    }

    pub fn get_bond(env: Env, feeder: Address) -> Option<Bond> {
        feeder_staking::get_bond(&env, &feeder)
    }

    pub fn set_dispute_window(env: Env, ledgers: u32) {
        Self::set_window_inner(&env, Auth::Admin, ledgers);
    }

    pub fn set_dispute_window_via_multisig(
        env: Env,
        multisig: Address,
        action_id: u64,
        ledgers: u32,
    ) {
        Self::set_window_inner(&env, Auth::Multisig(multisig, action_id), ledgers);
    }

    pub fn get_dispute_window(env: Env) -> u32 {
        feeder_staking::dispute_window(&env)
    }

    pub fn set_slash_destination(env: Env, destination: SlashDestination) {
        admin(&env, &Auth::Admin, ActionClass::Standard);
        env.storage()
            .instance()
            .set(&DataKey::SlashDestination, &destination);
    }

    /// `(burned, reward_pool)` totals of slashed bonds.
    pub fn get_slashed_totals(env: Env) -> (i128, i128) {
        let s = env.storage().instance();
        (
            s.get(&DataKey::Burned).unwrap_or(0),
            s.get(&DataKey::RewardPool).unwrap_or(0),
        )
    }

    /// Proposes slashing `amount` of `feeder`'s bond for the report that
    /// fed `report_id` (a round id); enters `Disputed` instead of slashing.
    /// This is also the hook the outlier filter's slash trigger calls.
    pub fn propose_slash(env: Env, feeder: Address, report_id: u64, amount: i128) {
        admin(&env, &Auth::Admin, ActionClass::Standard);
        feeder_staking::open(&env, &feeder, report_id, amount);
    }

    /// The accused feeder posts a (1..=64 byte) evidence hash before the
    /// window closes; a later submission replaces the earlier one.
    pub fn submit_evidence(env: Env, feeder: Address, evidence: Bytes) {
        ttl::extend_instance(&env);
        feeder.require_auth();
        feeder_staking::submit_evidence(&env, &feeder, evidence);
    }

    /// Admin/multisig verdict; fails with `DisputeWindowOpen` before the
    /// window has closed.
    pub fn resolve_dispute(env: Env, feeder: Address, resolution: Resolution) {
        admin(&env, &Auth::Admin, ActionClass::Standard);
        feeder_staking::resolve(&env, &feeder, resolution, 0);
    }

    pub fn resolve_dispute_via_multisig(
        env: Env,
        multisig: Address,
        action_id: u64,
        feeder: Address,
        resolution: Resolution,
    ) {
        admin(
            &env,
            &Auth::Multisig(multisig, action_id),
            ActionClass::Standard,
        );
        feeder_staking::resolve(&env, &feeder, resolution, 0);
    }

    /// Permissionless: restores the bond of a dispute nobody arbitrated
    /// once the window plus a one-day grace has passed.
    pub fn finalize_dispute(env: Env, feeder: Address) {
        ttl::extend_instance(&env);
        feeder_staking::resolve(
            &env,
            &feeder,
            Resolution::Restore,
            feeder_staking::FINALIZE_GRACE_LEDGERS,
        );
    }
}

impl PriceOracle {
    fn set_window_inner(env: &Env, auth: Auth, ledgers: u32) {
        admin(env, &auth, ActionClass::Standard);
        if ledgers == 0 {
            panic_with_error!(env, Error::InvalidDisputeWindow);
        }
        env.storage()
            .instance()
            .set(&DataKey::DisputeWindow, &ledgers);
    }
}

fn admin(env: &Env, auth: &Auth, class: ActionClass) {
    ttl::extend_instance(env);
    if !env.storage().instance().has(&DataKey::Admin) {
        panic_with_error!(env, Error::NotInitialized);
    }
    require_admin_or_multisig(env, &DataKey::Admin, auth, class, Error::Unauthorized);
}

fn max_staleness(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::MaxStaleness)
        .unwrap_or(3600)
}

fn min_reports(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::MinReports)
        .unwrap_or(1)
}

fn aggregate(env: &Env, symbol: &Symbol) -> Option<i128> {
    let now = env.ledger().timestamp();
    let stale_after = max_staleness(env);
    let check_disputes = feeder_staking::disputed_count(env) > 0;
    let mut prices = [0i128; math::MAX_FEEDERS as usize];
    let mut count = 0usize;
    for feeder in storage::feeder_list(env).iter() {
        if check_disputes && feeder_staking::is_disputed(env, &feeder) {
            continue;
        }
        let report: Option<(i128, u64)> = env
            .storage()
            .persistent()
            .get(&DataKey::PriceReport(symbol.clone(), feeder));
        if let Some((price, ts)) = report {
            if now.saturating_sub(ts) <= stale_after {
                prices[count] = price;
                count += 1;
            }
        }
    }
    if count == 0 || (count as u32) < min_reports(env) {
        return None;
    }
    Some(math::median(&mut prices, count))
}
