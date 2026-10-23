use soroban_sdk::{panic_with_error, symbol_short, Address, Env, Symbol, Vec};

use crate::error::Error;
use crate::math::{
    DEFAULT_FEE_RATE_BPS, EXPIRY_PAGE_SIZE, MAX_EXPIRY_BATCH, MAX_FEE_RATE_BPS, SETTLEMENT_WINDOW,
};
use crate::params_client;
use crate::ttl;
use crate::types::{DataKey, OptionSeries, OptionType, SeriesState};

/// Registry keys for the parameters options_market reads from params.
pub const FEE_RATE_PARAM: Symbol = symbol_short!("fee_bps");
pub const SETTLEMENT_WINDOW_PARAM: Symbol = symbol_short!("settle_w");

pub fn fee_rate_bps(env: &Env) -> i128 {
    sync_params(env);
    env.storage()
        .instance()
        .get(&DataKey::FeeRateBps)
        .unwrap_or(DEFAULT_FEE_RATE_BPS)
}

pub fn settlement_window(env: &Env) -> u64 {
    sync_params(env);
    env.storage()
        .instance()
        .get(&DataKey::SettlementWindow)
        .unwrap_or(SETTLEMENT_WINDOW)
}

/// Refreshes the locally cached registry values, but only when the
/// registry's version has moved — one cheap cross-contract call per read
/// instead of one per parameter. A registry that's unreachable, a
/// parameter that has never been set there, or a value outside this
/// contract's own hard limits all leave the cached value untouched.
fn sync_params(env: &Env) {
    let registry: Option<Address> = env.storage().instance().get(&DataKey::ParamsRegistry);
    let Some(registry) = registry else {
        return;
    };
    let client = params_client::Client::new(env, &registry);
    let Ok(Ok(version)) = client.try_get_version() else {
        return;
    };
    let cached: Option<u64> = env.storage().instance().get(&DataKey::ParamsVersion);
    if cached == Some(version) {
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
    let paused: bool = env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false);
    if paused {
        panic_with_error!(env, Error::ContractPaused);
    }
}

pub fn require_active_series(env: &Env, series_id: u64) -> OptionSeries {
    let series: OptionSeries = ttl::get_persistent(env, &DataKey::Series(series_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
    if series.state != SeriesState::Active {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    if env.ledger().timestamp() >= series.expiry {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    series
}

pub fn next_position_id(env: &Env) -> u64 {
    let counter: u64 = env
        .storage()
        .instance()
        .get(&DataKey::PositionCounter)
        .unwrap_or(0);
    let next = counter.checked_add(1).unwrap();
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

pub fn add_orphaned_liability(env: &Env, amount: i128) {
    if amount <= 0 {
        return;
    }
    let current: i128 = env
        .storage()
        .instance()
        .get(&DataKey::OrphanedLiabilities)
        .unwrap_or(0);
    env.storage().instance().set(
        &DataKey::OrphanedLiabilities,
        &current.checked_add(amount).unwrap(),
    );
}

pub fn deduct_orphaned_liability(env: &Env, amount: i128) {
    if amount <= 0 {
        return;
    }
    let current: i128 = env
        .storage()
        .instance()
        .get(&DataKey::OrphanedLiabilities)
        .unwrap_or(0);
    env.storage().instance().set(
        &DataKey::OrphanedLiabilities,
        &current.saturating_sub(amount),
    );
}

/// Reserves `(underlying, option_type, strike_price, expiry)` for
/// `series_id`, rejecting a spec that is already listed with
/// `DuplicateSeries`. The reservation lasts until `prune_series`.
pub fn claim_series_index(
    env: &Env,
    underlying: &Symbol,
    option_type: &OptionType,
    strike_price: i128,
    expiry: u64,
    series_id: u64,
) {
    let key = DataKey::SeriesIndex(
        underlying.clone(),
        option_type.clone(),
        strike_price,
        expiry,
    );
    if env.storage().persistent().has(&key) {
        panic_with_error!(env, Error::DuplicateSeries);
    }
    ttl::set_persistent(env, &key, &series_id);
}

// ─── Expiry calendar ─────────────────────────────────────────────────────────
//
// Series sharing `(underlying, expiry)` are listed in fixed-size pages
// (`ExpiryPage(underlying, expiry, n)`, `EXPIRY_PAGE_SIZE` ids each) plus a
// count, so walking `[cursor, cursor + limit)` touches at most two pages
// regardless of how many series share the expiry.

pub fn expiry_count(env: &Env, underlying: &Symbol, expiry: u64) -> u32 {
    ttl::get_persistent(env, &DataKey::ExpiryCount(underlying.clone(), expiry)).unwrap_or(0)
}

/// Appends `series` to its expiry's calendar once; later calls are no-ops.
pub fn index_expiry(env: &Env, series: &OptionSeries) {
    let indexed = DataKey::ExpiryIndexed(series.series_id);
    if env.storage().persistent().has(&indexed) {
        return;
    }
    let count = expiry_count(env, &series.underlying, series.expiry);
    let page_key = DataKey::ExpiryPage(
        series.underlying.clone(),
        series.expiry,
        count / EXPIRY_PAGE_SIZE,
    );
    let mut page: Vec<u64> = ttl::get_persistent(env, &page_key).unwrap_or_else(|| Vec::new(env));
    page.push_back(series.series_id);
    ttl::set_persistent(env, &page_key, &page);
    ttl::set_persistent(
        env,
        &DataKey::ExpiryCount(series.underlying.clone(), series.expiry),
        &(count + 1),
    );
    ttl::set_persistent(env, &indexed, &true);
}

/// Series ids at positions `[cursor, cursor + limit)` of the calendar.
pub fn expiry_page(
    env: &Env,
    underlying: &Symbol,
    expiry: u64,
    cursor: u32,
    limit: u32,
) -> Vec<u64> {
    let end = cursor
        .saturating_add(limit)
        .min(expiry_count(env, underlying, expiry));
    let mut out = Vec::new(env);
    let mut i = cursor;
    while i < end {
        let page_no = i / EXPIRY_PAGE_SIZE;
        let page: Vec<u64> = ttl::get_persistent(
            env,
            &DataKey::ExpiryPage(underlying.clone(), expiry, page_no),
        )
        .unwrap_or_else(|| Vec::new(env));
        let page_end = ((page_no + 1) * EXPIRY_PAGE_SIZE).min(end);
        while i < page_end {
            out.push_back(page.get(i % EXPIRY_PAGE_SIZE).unwrap());
            i += 1;
        }
    }
    out
}

pub fn require_expiry_limit(env: &Env, limit: u32) {
    if limit == 0 || limit > MAX_EXPIRY_BATCH {
        panic_with_error!(env, Error::InvalidBatchSize);
    }
}
