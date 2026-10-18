#![allow(dead_code)]

//! Storage helpers for the options market.
//!
//! All persistent reads/writes go through `ttl::get_persistent` /
//! `ttl::set_persistent` so TTL is extended on every access.

use soroban_sdk::{panic_with_error, Address, Env, Vec};

use crate::error::Error;
use crate::math::DEFAULT_FEE_RATE_BPS;
use crate::ttl;
use crate::types::{DataKey, OptionSeries, SeriesState};

// ─── Instance-storage helpers ────────────────────────────────────────────────

/// Current fee rate in basis points (defaults to `DEFAULT_FEE_RATE_BPS`).
pub fn fee_rate_bps(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::FeeRateBps)
        .unwrap_or(DEFAULT_FEE_RATE_BPS)
}

/// Premium pool balance (contract-wide, defaults 0).
pub fn get_premium_pool(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::PremiumPool)
        .unwrap_or(0i128)
}

/// Set premium pool balance.
pub fn set_premium_pool(env: &Env, amount: i128) {
    env.storage().instance().set(&DataKey::PremiumPool, &amount);
}

/// Lifetime series counter (monotonically increasing; used as the series id
/// for the next series AND as the total-series-created stat). Defaults to 0.
pub fn series_counter(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::SeriesCounter)
        .unwrap_or(0u64)
}

/// Increment the series counter and return the NEW value (= the new series id).
pub fn next_series_id(env: &Env) -> u64 {
    let id = series_counter(env) + 1;
    env.storage().instance().set(&DataKey::SeriesCounter, &id);
    id
}

/// Lifetime position counter. Defaults to 0.
pub fn position_counter(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::PositionCounter)
        .unwrap_or(0u64)
}

/// Increment the position counter and return the NEW value.
pub fn next_position_id(env: &Env) -> u64 {
    let id = position_counter(env) + 1;
    env.storage().instance().set(&DataKey::PositionCounter, &id);
    id
}

/// Number of series created for a given underlying (lifetime count).
pub fn series_count_for_underlying(env: &Env, underlying: &soroban_sdk::Symbol) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::SeriesCountForUnderlying(underlying.clone()))
        .unwrap_or(0u64)
}

/// Increment the series count for `underlying`.
pub fn increment_series_count(env: &Env, underlying: &soroban_sdk::Symbol) {
    let count = series_count_for_underlying(env, underlying);
    env.storage().instance().set(
        &DataKey::SeriesCountForUnderlying(underlying.clone()),
        &(count + 1),
    );
}

/// Cap on concurrently active series per underlying (stored in instance).
pub fn max_active_series(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::MaxActiveSeries)
        .unwrap_or(crate::math::MAX_SERIES_PER_UNDERLYING)
}

/// Active (slot-holding) series count for `underlying`.
pub fn active_series_count(env: &Env, underlying: &soroban_sdk::Symbol) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::ActiveSeriesCount(underlying.clone()))
        .unwrap_or(0u32)
}

/// Increment the active series count for `underlying`.
pub fn increment_active_count(env: &Env, underlying: &soroban_sdk::Symbol) {
    let n = active_series_count(env, underlying);
    env.storage().instance().set(
        &DataKey::ActiveSeriesCount(underlying.clone()),
        &(n + 1),
    );
}

/// Decrement the active series count for `underlying` (floors at 0).
pub fn decrement_active_count(env: &Env, underlying: &soroban_sdk::Symbol) {
    let n = active_series_count(env, underlying);
    env.storage().instance().set(
        &DataKey::ActiveSeriesCount(underlying.clone()),
        &n.saturating_sub(1),
    );
}

// ─── Persistent-storage helpers ──────────────────────────────────────────────

/// Remaining refund liability for a series (updated by buy/write/claim).
pub fn series_escrow(env: &Env, series_id: u64) -> i128 {
    ttl::get_persistent(env, &DataKey::SeriesEscrow(series_id)).unwrap_or(0i128)
}

/// Update the refund liability for a series.
pub fn set_series_escrow(env: &Env, series_id: u64, amount: i128) {
    ttl::set_persistent(env, &DataKey::SeriesEscrow(series_id), &amount);
}

// ─── Guard helpers ───────────────────────────────────────────────────────────

/// Load an `OptionSeries`, panicking with `SeriesNotFound` if absent, or
/// `SeriesNotActive` if the series is not in the `Active` state.
pub fn require_active_series(env: &Env, series_id: u64) -> OptionSeries {
    let series: OptionSeries =
        ttl::get_persistent(env, &DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
    if series.state != SeriesState::Active {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    series
}

/// Panic with `ContractPaused` if the contract is currently paused.
pub fn require_not_paused(env: &Env) {
    if zenith_common::is_paused(env, &DataKey::Paused) {
        panic_with_error!(env, Error::ContractPaused);
    }
}

// ─── User-position list ──────────────────────────────────────────────────────

/// Append `position_id` to `owner`'s persistent position list.
pub fn add_user_position(env: &Env, owner: &Address, position_id: u64) {
    let key = DataKey::UserPositions(owner.clone());
    let mut positions: Vec<u64> = ttl::get_persistent(env, &key).unwrap_or(Vec::new(env));
    positions.push_back(position_id);
    ttl::set_persistent(env, &key, &positions);
}
