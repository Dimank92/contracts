use soroban_sdk::{contracttype, Address, Env};

use crate::types::{DataKey, Position, Series};

/// Persistent storage TTL bump (ledgers). ~30 days at 5s ledgers.
pub const PERSISTENT_TTL: u32 = 518_400;
/// Threshold below which the TTL is extended on access.
pub const PERSISTENT_TTL_THRESHOLD: u32 = 259_200;

/// Read a series record, extending its TTL when present.
pub fn get_series(env: &Env, series_id: u64) -> Option<Series> {
    let key = DataKey::Series(series_id);
    let series: Option<Series> = env.storage().persistent().get(&key);
    if series.is_some() {
        env.storage()
            .persistent()
            .extend_ttl(&key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL);
    }
    series
}

/// Persist a series record and extend its TTL.
pub fn set_series(env: &Env, series_id: u64, series: &Series) {
    let key = DataKey::Series(series_id);
    env.storage().persistent().set(&key, series);
    env.storage()
        .persistent()
        .extend_ttl(&key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL);
}

/// Read a writer position keyed by `(series_id, writer)`, extending TTL.
pub fn get_position(env: &Env, series_id: u64, writer: &Address) -> Option<Position> {
    let key = DataKey::Position(series_id, writer.clone());
    let position: Option<Position> = env.storage().persistent().get(&key);
    if position.is_some() {
        env.storage()
            .persistent()
            .extend_ttl(&key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL);
    }
    position
}

/// Persist a writer position and extend its TTL.
pub fn set_position(env: &Env, series_id: u64, writer: &Address, position: &Position) {
    let key = DataKey::Position(series_id, writer.clone());
    env.storage().persistent().set(&key, position);
    env.storage()
        .persistent()
        .extend_ttl(&key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL);
}

/// Remove a writer position (used when a position is fully refunded).
pub fn remove_position(env: &Env, series_id: u64, writer: &Address) {
    let key = DataKey::Position(series_id, writer.clone());
    env.storage().persistent().remove(&key);
}

/// Read the paused flag, defaulting to `false` when unset.
pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

/// Persist the paused flag.
pub fn set_paused(env: &Env, paused: bool) {
    env.storage().instance().set(&DataKey::Paused, &paused);
}

/// Read the vault contract address, if configured.
pub fn get_vault(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Vault)
}

/// Persist the vault contract address.
pub fn set_vault(env: &Env, vault: &Address) {
    env.storage().instance().set(&DataKey::Vault, vault);
}

/// Read the next series id counter, defaulting to `0`.
pub fn next_series_id(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::NextSeriesId)
        .unwrap_or(0u64)
}

/// Persist the next series id counter.
pub fn set_next_series_id(env: &Env, id: u64) {
    env.storage().instance().set(&DataKey::NextSeriesId, &id);
}

/// Namespaced vault tag for a series' escrowed collateral.
#[contracttype]
#[derive(Clone)]
pub struct SeriesTag {
    pub namespace: soroban_sdk::Symbol,
    pub kind: soroban_sdk::Symbol,
    pub series_id: u64,
}

/// Build the `(options_market, "series", series_id)` vault tag.
pub fn series_tag(env: &Env, series_id: u64) -> SeriesTag {
    SeriesTag {
        namespace: soroban_sdk::Symbol::new(env, "options_market"),
        kind: soroban_sdk::Symbol::new(env, "series"),
        series_id,
    }
}
