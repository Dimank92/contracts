#![no_std]

//! Storage accessors for the options market. Every persistent read/write
//! goes through `ttl::get_persistent` / `ttl::set_persistent` so TTL
//! extension is automatic (see `ttl.rs` and the README "Storage TTL policy").

use soroban_sdk::{panic_with_error, Address, Env, Symbol, Vec};

use crate::error::Error;
use crate::math::DEFAULT_FEE_RATE_BPS;
use crate::ttl;
use crate::types::{DataKey, OptionPosition, OptionSeries, SeriesState};

// ─── Paused ───────────────────────────────────────────────────────────────────

/// Returns true when the contract is paused.
pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

/// Panics with `ContractPaused` if the contract is paused.
pub fn require_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, Error::ContractPaused);
    }
}

// ─── Admin ────────────────────────────────────────────────────────────────────

pub fn get_admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}

pub fn set_admin(env: &Env, admin: &Address) {
    env.storage().instance().set(&DataKey::Admin, admin);
}

pub fn get_oracle(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Oracle).unwrap()
}

pub fn get_collateral_token(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::CollateralToken)
        .unwrap()
}

pub fn get_fee_recipient(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::FeeRecipient)
        .unwrap()
}

// ─── Fee rate ─────────────────────────────────────────────────────────────────

/// Current fee rate in basis points (default `DEFAULT_FEE_RATE_BPS`).
pub fn fee_rate_bps(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::FeeRateBps)
        .unwrap_or(DEFAULT_FEE_RATE_BPS)
}

pub fn set_fee_rate_bps(env: &Env, bps: i128) {
    env.storage().instance().set(&DataKey::FeeRateBps, &bps);
}

// ─── Series counter ───────────────────────────────────────────────────────────

/// Lifetime number of series ever created (never decremented).
pub fn series_counter(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::SeriesCounter)
        .unwrap_or(0)
}

/// Allocates the next series id: reads SeriesCounter, increments, stores,
/// returns the NEW (post-increment) value.
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

// ─── Position counter ─────────────────────────────────────────────────────────

/// Allocates the next position id: reads PositionCounter, increments, stores,
/// returns the NEW (post-increment) value.
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

// ─── Series CRUD ─────────────────────────────────────────────────────────────

pub fn get_series(env: &Env, series_id: u64) -> Option<OptionSeries> {
    ttl::get_persistent(env, &DataKey::Series(series_id))
}

pub fn set_series(env: &Env, series: &OptionSeries) {
    ttl::set_persistent(env, &DataKey::Series(series.series_id), series);
}

/// Returns the series or panics with `SeriesNotFound`.
pub fn require_series(env: &Env, series_id: u64) -> OptionSeries {
    get_series(env, series_id)
        .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound))
}

/// Returns the series or panics with `SeriesNotFound`; also panics with
/// `SeriesNotActive` unless the series state is `Active`.
pub fn require_active_series(env: &Env, series_id: u64) -> OptionSeries {
    let series = require_series(env, series_id);
    if series.state != SeriesState::Active {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    series
}

// ─── Position CRUD ────────────────────────────────────────────────────────────

pub fn get_position(env: &Env, position_id: u64) -> Option<OptionPosition> {
    ttl::get_persistent(env, &DataKey::Position(position_id))
}

pub fn set_position(env: &Env, position: &OptionPosition) {
    ttl::set_persistent(env, &DataKey::Position(position.position_id), position);
}

/// Returns the position or panics with `PositionNotFound`.
pub fn require_position(env: &Env, position_id: u64) -> OptionPosition {
    get_position(env, position_id)
        .unwrap_or_else(|| panic_with_error!(env, Error::PositionNotFound))
}

// ─── User position lists ──────────────────────────────────────────────────────

pub fn get_user_positions(env: &Env, user: &Address) -> Vec<u64> {
    let key = DataKey::UserPositions(user.clone());
    env.storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env))
}

/// Appends `position_id` to `user`'s position list (no TTL here; the list
/// is read/updated through the instance path for efficiency in tests).
pub fn add_user_position(env: &Env, user: &Address, position_id: u64) {
    let key = DataKey::UserPositions(user.clone());
    let mut positions: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    positions.push_back(position_id);
    env.storage().persistent().set(&key, &positions);
    ttl::extend_persistent(env, &key);
}

// ─── Premium pool ─────────────────────────────────────────────────────────────

/// Contract-wide premium pool balance (net of fees, available to writers).
pub fn premium_pool(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::PremiumPool)
        .unwrap_or(0)
}

pub fn set_premium_pool(env: &Env, amount: i128) {
    env.storage().instance().set(&DataKey::PremiumPool, &amount);
}

// ─── Series escrow ────────────────────────────────────────────────────────────

/// Running total of refund-eligible principal still outstanding for `series_id`.
pub fn series_escrow(env: &Env, series_id: u64) -> i128 {
    ttl::get_persistent(env, &DataKey::SeriesEscrow(series_id)).unwrap_or(0)
}

pub fn set_series_escrow(env: &Env, series_id: u64, amount: i128) {
    ttl::set_persistent(env, &DataKey::SeriesEscrow(series_id), &amount);
}

// ─── Vault ────────────────────────────────────────────────────────────────────

/// The pinned trusted vault address (set by `set_vault`).
pub fn get_vault(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Vault)
}

pub fn set_vault(env: &Env, vault: &Address) {
    env.storage().instance().set(&DataKey::Vault, vault);
}

/// The vault address that holds escrow for `series_id` (set by
/// `escrow_series_to_vault`).
pub fn get_series_vault(env: &Env, series_id: u64) -> Option<Address> {
    ttl::get_persistent(env, &DataKey::SeriesVault(series_id))
}

pub fn set_series_vault(env: &Env, series_id: u64, vault: &Address) {
    ttl::set_persistent(env, &DataKey::SeriesVault(series_id), vault);
}

// ─── Underlying price ─────────────────────────────────────────────────────────

pub fn get_underlying_price(env: &Env, underlying: &Symbol) -> Option<i128> {
    ttl::get_persistent(env, &DataKey::UnderlyingPrice(underlying.clone()))
}

pub fn set_underlying_price(env: &Env, underlying: &Symbol, price: i128) {
    ttl::set_persistent(env, &DataKey::UnderlyingPrice(underlying.clone()), &price);
}

// ─── Open interest / premiums collected ──────────────────────────────────────

pub fn total_open_interest(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalOpenInterest)
        .unwrap_or(0)
}

pub fn set_total_open_interest(env: &Env, oi: i128) {
    env.storage()
        .instance()
        .set(&DataKey::TotalOpenInterest, &oi);
}

pub fn total_premiums_collected(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalPremiumsCollected)
        .unwrap_or(0)
}

pub fn set_total_premiums_collected(env: &Env, amt: i128) {
    env.storage()
        .instance()
        .set(&DataKey::TotalPremiumsCollected, &amt);
}

// ─── Series count per underlying ─────────────────────────────────────────────

pub fn series_count_for_underlying(env: &Env, underlying: &Symbol) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::SeriesCountForUnderlying(underlying.clone()))
        .unwrap_or(0)
}

pub fn set_series_count_for_underlying(env: &Env, underlying: &Symbol, count: u64) {
    env.storage()
        .instance()
        .set(&DataKey::SeriesCountForUnderlying(underlying.clone()), &count);
}
