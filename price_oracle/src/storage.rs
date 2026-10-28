use soroban_sdk::{Address, Env, Symbol, Vec};

use crate::ttl;
use crate::types::DataKey;

// Feeders (issue #101): moved out of instance storage — which is loaded in
// full on every invocation, including the hot `report_price`/`get_price`
// paths — into dedicated persistent entries, with an O(1) membership map
// instead of `Vec::contains`. Reads and writes go through the ttl helpers
// so live feeder entries are kept alive by normal use.

pub fn is_feeder(env: &Env, feeder: &Address) -> bool {
    ttl::get_persistent(env, &DataKey::IsFeeder(feeder.clone())).unwrap_or(false)
}

pub fn feeder_list(env: &Env) -> Vec<Address> {
    ttl::get_persistent(env, &DataKey::Feeders).unwrap_or(Vec::new(env))
}

pub fn add_feeder(env: &Env, feeder: &Address) {
    ttl::set_persistent(env, &DataKey::IsFeeder(feeder.clone()), &true);
    let mut list = feeder_list(env);
    list.push_back(feeder.clone());
    ttl::set_persistent(env, &DataKey::Feeders, &list);
}

pub fn remove_feeder(env: &Env, feeder: &Address) {
    env.storage()
        .persistent()
        .remove(&DataKey::IsFeeder(feeder.clone()));
    let list = feeder_list(env);
    if let Some(idx) = list.iter().position(|f| &f == feeder) {
        let mut list = list;
        list.remove(idx as u32);
        ttl::set_persistent(env, &DataKey::Feeders, &list);
    }
}

pub fn replace_feeder(env: &Env, old_feeder: &Address, new_feeder: &Address) {
    env.storage()
        .persistent()
        .remove(&DataKey::IsFeeder(old_feeder.clone()));
    ttl::set_persistent(env, &DataKey::IsFeeder(new_feeder.clone()), &true);
    let list = feeder_list(env);
    if let Some(idx) = list.iter().position(|f| &f == old_feeder) {
        let mut list = list;
        list.set(idx as u32, new_feeder.clone());
        ttl::set_persistent(env, &DataKey::Feeders, &list);
    }
}

// ─── Per-symbol Feeders (issue #200) ─────────────────────────────────────────

pub fn is_feeder_for_symbol(env: &Env, symbol: &Symbol, feeder: &Address) -> bool {
    ttl::get_persistent(
        env,
        &DataKey::IsSymbolFeeder(symbol.clone(), feeder.clone()),
    )
    .unwrap_or(false)
}

pub fn symbol_feeder_list(env: &Env, symbol: &Symbol) -> Vec<Address> {
    ttl::get_persistent(env, &DataKey::SymbolFeeders(symbol.clone())).unwrap_or(Vec::new(env))
}

pub fn add_symbol_feeder(env: &Env, symbol: &Symbol, feeder: &Address) {
    ttl::set_persistent(
        env,
        &DataKey::IsSymbolFeeder(symbol.clone(), feeder.clone()),
        &true,
    );
    let mut list = symbol_feeder_list(env, symbol);
    list.push_back(feeder.clone());
    ttl::set_persistent(env, &DataKey::SymbolFeeders(symbol.clone()), &list);
}

pub fn remove_symbol_feeder(env: &Env, symbol: &Symbol, feeder: &Address) {
    env.storage()
        .persistent()
        .remove(&DataKey::IsSymbolFeeder(symbol.clone(), feeder.clone()));
    let list = symbol_feeder_list(env, symbol);
    if let Some(idx) = list.iter().position(|f| &f == feeder) {
        let mut list = list;
        list.remove(idx as u32);
        ttl::set_persistent(env, &DataKey::SymbolFeeders(symbol.clone()), &list);
    }
}

pub fn replace_symbol_feeder(
    env: &Env,
    symbol: &Symbol,
    old_feeder: &Address,
    new_feeder: &Address,
) {
    env.storage()
        .persistent()
        .remove(&DataKey::IsSymbolFeeder(symbol.clone(), old_feeder.clone()));
    ttl::set_persistent(
        env,
        &DataKey::IsSymbolFeeder(symbol.clone(), new_feeder.clone()),
        &true,
    );
    let list = symbol_feeder_list(env, symbol);
    if let Some(idx) = list.iter().position(|f| &f == old_feeder) {
        let mut list = list;
        list.set(idx as u32, new_feeder.clone());
        ttl::set_persistent(env, &DataKey::SymbolFeeders(symbol.clone()), &list);
    }
}

pub fn effective_feeder_list(env: &Env, symbol: &Symbol) -> Vec<Address> {
    let sym_list = symbol_feeder_list(env, symbol);
    if !sym_list.is_empty() {
        sym_list
    } else {
        feeder_list(env)
    }
}

pub fn is_effective_feeder(env: &Env, symbol: &Symbol, feeder: &Address) -> bool {
    let sym_list = symbol_feeder_list(env, symbol);
    if !sym_list.is_empty() {
        is_feeder_for_symbol(env, symbol, feeder)
    } else {
        is_feeder(env, feeder)
    }
}

// Quorum and staleness configuration (issue #227): the minimum number of
// fresh feeder reports required before an aggregate may be produced, and the
// staleness window that decides which reports still count as fresh. Both are
// persisted through the shared TTL helpers so every read/write extends the
// entry's lifetime.

#[allow(dead_code)]
pub fn get_min_reports(env: &Env) -> u32 {
    crate::ttl::get_persistent(env, &DataKey::MinReports).unwrap_or(0)
}

#[allow(dead_code)]
pub fn set_min_reports(env: &Env, min_reports: u32) {
    crate::ttl::set_persistent(env, &DataKey::MinReports, &min_reports);
}

#[allow(dead_code)]
pub fn get_max_staleness(env: &Env) -> u64 {
    crate::ttl::get_persistent(env, &DataKey::MaxStaleness).unwrap_or(0)
}

#[allow(dead_code)]
pub fn set_max_staleness(env: &Env, max_staleness: u64) {
    crate::ttl::set_persistent(env, &DataKey::MaxStaleness, &max_staleness);
}

/// Returns the number of currently-fresh feeder reports for `symbol`.
///
/// A report is fresh when `now - timestamp <= max_staleness`, where `now` is
/// read from `env.ledger().timestamp()` (never from a caller-supplied value).
/// Reports whose timestamp is in the future relative to `now` are treated as
/// fresh (they cannot be stale yet). When no staleness window is configured
/// (i.e. it is zero) no report is considered fresh, so aggregation fails
/// closed.
#[allow(dead_code)]
pub fn fresh_report_count(env: &Env, symbol: &soroban_sdk::Symbol) -> u32 {
    let max_staleness = get_max_staleness(env);
    if max_staleness == 0 {
        return 0;
    }

    let now = env.ledger().timestamp();
    let mut count: u32 = 0;

    for feeder in effective_feeder_list(env, symbol).iter() {
        if let Some((_price, timestamp)) =
            crate::oracle_adapter::latest_report(env, symbol, &feeder)
        {
            let fresh = if timestamp > now {
                true
            } else {
                now.saturating_sub(timestamp) <= max_staleness
            };
            if fresh {
                count += 1;
            }
        }
    }

    count
}
