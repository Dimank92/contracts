use soroban_sdk::{panic_with_error, symbol_short, token, Address, Env, Symbol, Vec};

use crate::error::Error;
use crate::math::{DEFAULT_FEE_RATE_BPS, MAX_FEE_RATE_BPS, SETTLEMENT_WINDOW};
use crate::params_client;
use crate::ttl;
use crate::types::{DataKey, OptionPosition, OptionSeries, SeriesState};

pub const FEE_RATE_PARAM: Symbol = symbol_short!("fee_bps");
pub const SETTLEMENT_WINDOW_PARAM: Symbol = symbol_short!("settle_w");

pub fn instance_get<V: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>>(
    env: &Env,
    key: &DataKey,
) -> Option<V> {
    env.storage().instance().get(key)
}

pub fn instance_i128(env: &Env, key: &DataKey) -> i128 {
    instance_get(env, key).unwrap_or(0)
}

pub fn collateral_token(env: &Env) -> token::Client<'_> {
    let address: Address = instance_get(env, &DataKey::CollateralToken)
        .unwrap_or_else(|| panic_with_error!(env, Error::Unauthorized));
    token::Client::new(env, &address)
}

/// Adds `delta` (which may be negative) to an i128 counter in instance
/// storage, failing loudly on overflow instead of wrapping.
pub fn bump_instance_i128(env: &Env, key: &DataKey, delta: i128) {
    let next = instance_i128(env, key)
        .checked_add(delta)
        .unwrap_or_else(|| panic_with_error!(env, Error::InvalidSeriesParams));
    env.storage().instance().set(key, &next);
}

pub fn fee_rate_bps(env: &Env) -> i128 {
    sync_params(env);
    instance_get(env, &DataKey::FeeRateBps).unwrap_or(DEFAULT_FEE_RATE_BPS)
}

pub fn settlement_window(env: &Env) -> u64 {
    sync_params(env);
    instance_get(env, &DataKey::SettlementWindow).unwrap_or(SETTLEMENT_WINDOW)
}

/// Refreshes the cached fee rate / settlement window from the params
/// registry when its version moved. An unreachable registry, an unset
/// parameter or an out-of-range value leaves the cached value in place.
fn sync_params(env: &Env) {
    let Some(registry) = instance_get::<Address>(env, &DataKey::ParamsRegistry) else {
        return;
    };
    let client = params_client::Client::new(env, &registry);
    let Ok(Ok(version)) = client.try_get_version() else {
        return;
    };
    if instance_get::<u64>(env, &DataKey::ParamsVersion) == Some(version) {
        return;
    }
    if let Ok(Ok(Some(param))) = client.try_get_param(&FEE_RATE_PARAM) {
        if (0..=MAX_FEE_RATE_BPS).contains(&param.value) {
            env.storage()
                .instance()
                .set(&DataKey::FeeRateBps, &param.value);
        }
    }
    if let Ok(Ok(Some(param))) = client.try_get_param(&SETTLEMENT_WINDOW_PARAM) {
        if param.value > 0 && param.value <= u64::MAX as i128 {
            env.storage()
                .instance()
                .set(&DataKey::SettlementWindow, &(param.value as u64));
        }
    }
    env.storage()
        .instance()
        .set(&DataKey::ParamsVersion, &version);
}

pub fn require_not_paused(env: &Env) {
    zenith_common::require_not_paused(env, &DataKey::Paused, Error::ContractPaused);
}

pub fn get_series(env: &Env, series_id: u64) -> OptionSeries {
    ttl::get_persistent(env, &DataKey::Series(series_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound))
}

pub fn put_series(env: &Env, series: &OptionSeries) {
    ttl::set_persistent(env, &DataKey::Series(series.series_id), series);
}

pub fn get_position(env: &Env, position_id: u64) -> OptionPosition {
    ttl::get_persistent(env, &DataKey::Position(position_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::PositionNotFound))
}

pub fn put_position(env: &Env, position: &OptionPosition) {
    ttl::set_persistent(env, &DataKey::Position(position.position_id), position);
}

pub fn require_active_series(env: &Env, series_id: u64) -> OptionSeries {
    let series = get_series(env, series_id);
    if series.state != SeriesState::Active || env.ledger().timestamp() >= series.expiry {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    series
}

pub fn next_position_id(env: &Env) -> u64 {
    let counter: u64 = instance_get(env, &DataKey::PositionCounter).unwrap_or(0);
    let next = counter
        .checked_add(1)
        .unwrap_or_else(|| panic_with_error!(env, Error::InvalidSeriesParams));
    env.storage()
        .instance()
        .set(&DataKey::PositionCounter, &next);
    next
}

pub fn add_user_position(env: &Env, user: &Address, position_id: u64) {
    let key = DataKey::UserPositions(user.clone());
    let mut positions: Vec<u64> = ttl::get_persistent(env, &key).unwrap_or_else(|| Vec::new(env));
    positions.push_back(position_id);
    ttl::set_persistent(env, &key, &positions);
}

/// The vault holding `series_id`'s writer collateral, if the series was
/// created against a pinned vault.
pub fn collateral_vault(env: &Env, series_id: u64) -> Option<Address> {
    let in_vault: bool =
        ttl::get_persistent(env, &DataKey::CollateralInVault(series_id)).unwrap_or(false);
    if in_vault {
        ttl::get_persistent(env, &DataKey::SeriesVault(series_id))
    } else {
        None
    }
}
