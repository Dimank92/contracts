use soroban_sdk::{Address, Env, Symbol};

use crate::types::SymbolMeta;

pub fn admin_transferred(env: &Env, old_admin: Address, new_admin: Address) {
    env.events().publish(
        (Symbol::new(env, "admin_transferred"),),
        (old_admin, new_admin),
    );
}

pub fn paused(env: &Env) {
    env.events().publish((Symbol::new(env, "paused"),), ());
}

pub fn unpaused(env: &Env) {
    env.events().publish((Symbol::new(env, "unpaused"),), ());
}

pub fn max_staleness_updated(env: &Env, new_staleness: u64) {
    env.events()
        .publish((Symbol::new(env, "max_staleness_updated"),), new_staleness);
}

pub fn min_reports_updated(env: &Env, new_min_reports: u32) {
    env.events()
        .publish((Symbol::new(env, "min_reports_updated"),), new_min_reports);
}

pub fn feeder_added(env: &Env, feeder: Address) {
    env.events()
        .publish((Symbol::new(env, "feeder_added"),), feeder);
}

pub fn feeder_removed(env: &Env, feeder: Address) {
    env.events()
        .publish((Symbol::new(env, "feeder_removed"),), feeder);
}

pub fn price_reported(env: &Env, feeder: Address, symbol: Symbol, price: i128) {
    env.events()
        .publish((Symbol::new(env, "price_reported"), feeder, symbol), price);
}

/// Emitted when the admin registers a new symbol in the oracle registry.
/// `data` is the `SymbolMeta` the symbol was registered with.
pub fn symbol_registered(env: &Env, symbol: Symbol, meta: SymbolMeta) {
    env.events()
        .publish((Symbol::new(env, "symbol_registered"), symbol), meta);
}

/// Emitted when the admin removes a symbol from the oracle registry.
/// Outstanding aggregated price data is NOT deleted — it remains
/// readable via `get_latest_report` for audit purposes but `get_price`
/// will refuse to aggregate it (the symbol check in `report_price` now
/// rejects updates, so existing reports only age out naturally).
pub fn symbol_removed(env: &Env, symbol: Symbol) {
    env.events()
        .publish((Symbol::new(env, "symbol_removed"),), symbol);
}
