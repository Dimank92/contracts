//! Typed storage accessors for the options market contract.
//!
//! Every public function here corresponds to one or more `DataKey` variants
//! from `types.rs`. Instance-storage keys (admin, oracle, token, fee
//! recipient, counters, global accumulators, pause flag, fee rate, premium
//! pool) are read/written directly via `env.storage().instance()`.
//! Series- and position-scoped records are persistent and always go through
//! `ttl::get_persistent` / `ttl::set_persistent` so their TTL is extended
//! on every access (see the "Storage TTL policy" table in the README).

use soroban_sdk::{Address, Env, Symbol, Vec};

use crate::ttl;
use crate::types::{DataKey, OptionPosition, OptionSeries};

// ─── instance-storage helpers ───────────────────────────────────────────────

/// Initialise admin, oracle, collateral token and fee recipient once at
/// deploy time.  Panics with `AlreadyInitialized` if the admin key already
/// exists.
pub fn initialize(
    env: &Env,
    admin: &Address,
    oracle: &Address,
    collateral_token: &Address,
    fee_recipient: &Address,
) {
    env.storage().instance().set(&DataKey::Admin, admin);
    env.storage().instance().set(&DataKey::Oracle, oracle);
    env.storage()
        .instance()
        .set(&DataKey::CollateralToken, collateral_token);
    env.storage()
        .instance()
        .set(&DataKey::FeeRecipient, fee_recipient);
    // Zero out all running totals so views never see stale values.
    env.storage()
        .instance()
        .set(&DataKey::SeriesCounter, &0u64);
    env.storage()
        .instance()
        .set(&DataKey::PositionCounter, &0u64);
    env.storage()
        .instance()
        .set(&DataKey::TotalPremiumsCollected, &0i128);
    env.storage()
        .instance()
        .set(&DataKey::TotalOpenInterest, &0i128);
    env.storage()
        .instance()
        .set(&DataKey::Paused, &false);
    env.storage()
        .instance()
        .set(&DataKey::FeeRateBps, &crate::math::DEFAULT_FEE_RATE_BPS);
    env.storage()
        .instance()
        .set(&DataKey::PremiumPool, &0i128);
}

/// Returns `true` if the contract has been initialised (admin key present).
pub fn is_initialized(env: &Env) -> bool {
    env.storage().instance().has(&DataKey::Admin)
}

// ─── admin / oracle / token / fee recipient ─────────────────────────────────

pub fn admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}

pub fn set_admin(env: &Env, new_admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, new_admin);
}

pub fn oracle(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Oracle).unwrap()
}

pub fn collateral_token(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::CollateralToken)
        .unwrap()
}

pub fn fee_recipient(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::FeeRecipient)
        .unwrap()
}

// ─── pause ──────────────────────────────────────────────────────────────────

pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

pub fn set_paused(env: &Env, paused: bool) {
    env.storage().instance().set(&DataKey::Paused, &paused);
}

/// Panics with `Error::ContractPaused` if the contract is currently paused.
pub fn require_not_paused(env: &Env) {
    if is_paused(env) {
        soroban_sdk::panic_with_error!(env, crate::error::Error::ContractPaused);
    }
}

// ─── fee rate ───────────────────────────────────────────────────────────────

pub fn fee_rate_bps(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::FeeRateBps)
        .unwrap_or(crate::math::DEFAULT_FEE_RATE_BPS)
}

pub fn set_fee_rate_bps(env: &Env, bps: i128) {
    env.storage().instance().set(&DataKey::FeeRateBps, &bps);
}

// ─── premium pool ────────────────────────────────────────────────────────────

pub fn premium_pool(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::PremiumPool)
        .unwrap_or(0)
}

pub fn set_premium_pool(env: &Env, amount: i128) {
    env.storage()
        .instance()
        .set(&DataKey::PremiumPool, &amount);
}

// ─── global accumulators ────────────────────────────────────────────────────

pub fn total_premiums_collected(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalPremiumsCollected)
        .unwrap_or(0)
}

pub fn set_total_premiums_collected(env: &Env, value: i128) {
    env.storage()
        .instance()
        .set(&DataKey::TotalPremiumsCollected, &value);
}

pub fn total_open_interest(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalOpenInterest)
        .unwrap_or(0)
}

pub fn set_total_open_interest(env: &Env, value: i128) {
    env.storage()
        .instance()
        .set(&DataKey::TotalOpenInterest, &value);
}

// ─── counters ────────────────────────────────────────────────────────────────

/// Atomically increments the series counter and returns the **new** id.
pub fn next_series_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::SeriesCounter)
        .unwrap_or(0)
        + 1;
    env.storage().instance().set(&DataKey::SeriesCounter, &id);
    id
}

/// Returns the current series counter (total series ever created).
pub fn series_counter(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::SeriesCounter)
        .unwrap_or(0)
}

/// Atomically increments the position counter and returns the **new** id.
pub fn next_position_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::PositionCounter)
        .unwrap_or(0)
        + 1;
    env.storage()
        .instance()
        .set(&DataKey::PositionCounter, &id);
    id
}

// ─── per-underlying series count ─────────────────────────────────────────────

/// Returns the number of series (ever) created for `underlying`.
pub fn series_count_for_underlying(env: &Env, underlying: &Symbol) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::SeriesCountForUnderlying(underlying.clone()))
        .unwrap_or(0u32)
}

pub fn increment_series_count_for_underlying(env: &Env, underlying: &Symbol) {
    let count = series_count_for_underlying(env, underlying) + 1;
    env.storage().persistent().set(
        &DataKey::SeriesCountForUnderlying(underlying.clone()),
        &count,
    );
}

// ─── underlying price ────────────────────────────────────────────────────────

pub fn underlying_price(env: &Env, underlying: &Symbol) -> Option<i128> {
    ttl::get_persistent(env, &DataKey::UnderlyingPrice(underlying.clone()))
}

pub fn set_underlying_price(env: &Env, underlying: &Symbol, price: i128) {
    ttl::set_persistent(env, &DataKey::UnderlyingPrice(underlying.clone()), &price);
}

// ─── series ──────────────────────────────────────────────────────────────────

/// Returns the series record for `series_id`, or `None` if not found.
pub fn get_series(env: &Env, series_id: u64) -> Option<OptionSeries> {
    ttl::get_persistent(env, &DataKey::Series(series_id))
}

/// Panics with `Error::SeriesNotFound` if the series does not exist.
pub fn require_series(env: &Env, series_id: u64) -> OptionSeries {
    get_series(env, series_id)
        .unwrap_or_else(|| soroban_sdk::panic_with_error!(env, crate::error::Error::SeriesNotFound))
}

/// Panics with `Error::SeriesNotActive` if the series is not in `Active` state.
pub fn require_active_series(env: &Env, series_id: u64) -> OptionSeries {
    use crate::types::SeriesState;
    let series = require_series(env, series_id);
    if series.state != SeriesState::Active {
        soroban_sdk::panic_with_error!(env, crate::error::Error::SeriesNotActive);
    }
    series
}

pub fn set_series(env: &Env, series_id: u64, series: &OptionSeries) {
    ttl::set_persistent(env, &DataKey::Series(series_id), series);
}

pub fn remove_series(env: &Env, series_id: u64) {
    env.storage().persistent().remove(&DataKey::Series(series_id));
}

// ─── position ────────────────────────────────────────────────────────────────

/// Returns the position record for `position_id`, or `None` if not found.
pub fn get_position(env: &Env, position_id: u64) -> Option<OptionPosition> {
    ttl::get_persistent(env, &DataKey::Position(position_id))
}

/// Panics with `Error::PositionNotFound` if the position does not exist.
pub fn require_position(env: &Env, position_id: u64) -> OptionPosition {
    get_position(env, position_id).unwrap_or_else(|| {
        soroban_sdk::panic_with_error!(env, crate::error::Error::PositionNotFound)
    })
}

pub fn set_position(env: &Env, position_id: u64, position: &OptionPosition) {
    ttl::set_persistent(env, &DataKey::Position(position_id), position);
}

pub fn remove_position(env: &Env, position_id: u64) {
    env.storage()
        .persistent()
        .remove(&DataKey::Position(position_id));
}

// ─── user position list ──────────────────────────────────────────────────────

/// Returns the list of position ids owned by `user`.
pub fn user_positions(env: &Env, user: &Address) -> Vec<u64> {
    ttl::get_persistent(env, &DataKey::UserPositions(user.clone()))
        .unwrap_or(Vec::new(env))
}

/// Appends `position_id` to `user`'s position list.
pub fn add_user_position(env: &Env, user: &Address, position_id: u64) {
    let mut positions = user_positions(env, user);
    positions.push_back(position_id);
    ttl::set_persistent(env, &DataKey::UserPositions(user.clone()), &positions);
}

/// Removes `position_id` from `user`'s position list.
pub fn remove_user_position(env: &Env, user: &Address, position_id: u64) {
    let positions = user_positions(env, user);
    let mut updated: Vec<u64> = Vec::new(env);
    for id in positions.iter() {
        if id != position_id {
            updated.push_back(id);
        }
    }
    ttl::set_persistent(env, &DataKey::UserPositions(user.clone()), &updated);
}

// ─── series escrow (running refund liability) ────────────────────────────────

/// Returns the outstanding refund liability for a series (sum of all
/// yet-unclaimed `premium_net_of_fee` for long positions plus full
/// `collateral_locked` for short positions in this series).
pub fn series_escrow(env: &Env, series_id: u64) -> i128 {
    ttl::get_persistent(env, &DataKey::SeriesEscrow(series_id)).unwrap_or(0)
}

/// Adds `amount` to the series' running escrow liability.
pub fn increase_series_escrow(env: &Env, series_id: u64, amount: i128) {
    let current = series_escrow(env, series_id);
    ttl::set_persistent(
        env,
        &DataKey::SeriesEscrow(series_id),
        &(current + amount),
    );
}

/// Subtracts `amount` from the series' running escrow liability.
pub fn decrease_series_escrow(env: &Env, series_id: u64, amount: i128) {
    let current = series_escrow(env, series_id);
    let next = (current - amount).max(0);
    ttl::set_persistent(env, &DataKey::SeriesEscrow(series_id), &next);
}

/// Zeroes the series escrow and returns the previous value (used by
/// `escrow_series_to_vault` to transfer exactly the right amount).
pub fn take_series_escrow(env: &Env, series_id: u64) -> i128 {
    let amount = series_escrow(env, series_id);
    ttl::set_persistent(env, &DataKey::SeriesEscrow(series_id), &0i128);
    amount
}

pub fn remove_series_escrow(env: &Env, series_id: u64) {
    env.storage()
        .persistent()
        .remove(&DataKey::SeriesEscrow(series_id));
}

// ─── vault pin ───────────────────────────────────────────────────────────────

pub fn vault(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Vault)
}

pub fn set_vault(env: &Env, vault: &Address) {
    env.storage().instance().set(&DataKey::Vault, vault);
}

/// Returns which vault address holds this series' escrow, if any.
pub fn series_vault(env: &Env, series_id: u64) -> Option<Address> {
    ttl::get_persistent(env, &DataKey::SeriesVault(series_id))
}

pub fn set_series_vault(env: &Env, series_id: u64, vault: &Address) {
    ttl::set_persistent(env, &DataKey::SeriesVault(series_id), vault);
}

pub fn remove_series_vault(env: &Env, series_id: u64) {
    env.storage()
        .persistent()
        .remove(&DataKey::SeriesVault(series_id));
}
