use soroban_sdk::{panic_with_error, Address, Env, Vec};

use crate::error::Error;
use crate::math::DEFAULT_FEE_RATE_BPS;
use crate::ttl;
use crate::types::{DataKey, OptionPosition, OptionSeries, SeriesState};

// ─── admin helpers ────────────────────────────────────────────────────────────

pub fn require_not_paused(env: &Env) {
    zenith_common::require_not_paused(env, &DataKey::Paused, Error::ContractPaused);
}

pub fn require_active_series(env: &Env, series_id: u64) -> OptionSeries {
    let series = get_series(env, series_id);
    if series.state != SeriesState::Active {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    series
}

// ─── counter helpers ──────────────────────────────────────────────────────────

pub fn next_series_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::SeriesCounter)
        .unwrap_or(0u64)
        + 1;
    env.storage().instance().set(&DataKey::SeriesCounter, &id);
    id
}

pub fn next_position_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::PositionCounter)
        .unwrap_or(0u64)
        + 1;
    env.storage()
        .instance()
        .set(&DataKey::PositionCounter, &id);
    id
}

// ─── fee rate ─────────────────────────────────────────────────────────────────

pub fn fee_rate_bps(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::FeeRateBps)
        .unwrap_or(DEFAULT_FEE_RATE_BPS)
}

// ─── series ───────────────────────────────────────────────────────────────────

pub fn get_series(env: &Env, series_id: u64) -> OptionSeries {
    ttl::get_persistent(env, &DataKey::Series(series_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound))
}

pub fn save_series(env: &Env, series: &OptionSeries) {
    ttl::set_persistent(env, &DataKey::Series(series.series_id), series);
}

pub fn series_count_for_underlying(env: &Env, underlying: &soroban_sdk::Symbol) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::SeriesCountForUnderlying(underlying.clone()))
        .unwrap_or(0u32)
}

pub fn increment_series_count(env: &Env, underlying: &soroban_sdk::Symbol) {
    let count = series_count_for_underlying(env, underlying) + 1;
    env.storage()
        .instance()
        .set(&DataKey::SeriesCountForUnderlying(underlying.clone()), &count);
}

// ─── position ─────────────────────────────────────────────────────────────────

pub fn get_position(env: &Env, position_id: u64) -> OptionPosition {
    ttl::get_persistent(env, &DataKey::Position(position_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::PositionNotFound))
}

pub fn save_position(env: &Env, position: &OptionPosition) {
    ttl::set_persistent(env, &DataKey::Position(position.position_id), position);
}

pub fn add_user_position(env: &Env, owner: &Address, position_id: u64) {
    let mut positions: Vec<u64> = ttl::get_persistent(env, &DataKey::UserPositions(owner.clone()))
        .unwrap_or(Vec::new(env));
    positions.push_back(position_id);
    ttl::set_persistent(env, &DataKey::UserPositions(owner.clone()), &positions);
}

pub fn get_user_positions(env: &Env, owner: &Address) -> Vec<u64> {
    ttl::get_persistent(env, &DataKey::UserPositions(owner.clone())).unwrap_or(Vec::new(env))
}

// ─── premium pool ─────────────────────────────────────────────────────────────

pub fn premium_pool(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::PremiumPool)
        .unwrap_or(0i128)
}

pub fn set_premium_pool(env: &Env, amount: i128) {
    env.storage().instance().set(&DataKey::PremiumPool, &amount);
}

// ─── series escrow ────────────────────────────────────────────────────────────

pub fn get_series_escrow(env: &Env, series_id: u64) -> i128 {
    ttl::get_persistent(env, &DataKey::SeriesEscrow(series_id)).unwrap_or(0i128)
}

pub fn set_series_escrow(env: &Env, series_id: u64, amount: i128) {
    ttl::set_persistent(env, &DataKey::SeriesEscrow(series_id), &amount);
}

// ─── stats ───────────────────────────────────────────────────────────────────

pub fn total_premiums(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalPremiumsCollected)
        .unwrap_or(0i128)
}

pub fn add_premium(env: &Env, amount: i128) {
    let t = total_premiums(env) + amount;
    env.storage()
        .instance()
        .set(&DataKey::TotalPremiumsCollected, &t);
}

pub fn total_open_interest(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalOpenInterest)
        .unwrap_or(0i128)
}

pub fn add_open_interest(env: &Env, amount: i128) {
    let t = total_open_interest(env) + amount;
    env.storage()
        .instance()
        .set(&DataKey::TotalOpenInterest, &t);
}
