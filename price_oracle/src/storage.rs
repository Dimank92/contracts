use soroban_sdk::{Address, Env, Vec};

use crate::types::DataKey;

// Feeders (issue #101): moved out of instance storage — which is loaded in
// full on every invocation, including the hot `report_price`/`get_price`
// paths — into dedicated persistent entries, with an O(1) membership map
// instead of `Vec::contains`.

pub fn is_feeder(env: &Env, feeder: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::IsFeeder(feeder.clone()))
        .unwrap_or(false)
}

pub fn feeder_list(env: &Env) -> Vec<Address> {
    env.storage()
        .persistent()
        .get(&DataKey::Feeders)
        .unwrap_or(Vec::new(env))
}

pub fn add_feeder(env: &Env, feeder: &Address) {
    env.storage()
        .persistent()
        .set(&DataKey::IsFeeder(feeder.clone()), &true);
    let mut list = feeder_list(env);
    list.push_back(feeder.clone());
    env.storage().persistent().set(&DataKey::Feeders, &list);
}

pub fn remove_feeder(env: &Env, feeder: &Address) {
    env.storage()
        .persistent()
        .remove(&DataKey::IsFeeder(feeder.clone()));
    let list = feeder_list(env);
    if let Some(idx) = list.iter().position(|f| &f == feeder) {
        let mut list = list;
        list.remove(idx as u32);
        env.storage().persistent().set(&DataKey::Feeders, &list);
    }
}

// Quorum and staleness configuration (issue #227): the minimum number of
// fresh feeder reports required before an aggregate may be produced, and the
// staleness window that decides which reports still count as fresh. Both are
// persisted through the shared TTL helpers so every read/write extends the
// entry's lifetime.

pub fn get_min_reports(env: &Env) -> u32 {
    crate::ttl::get_persistent(env, &DataKey::MinReports).unwrap_or(0)
}

pub fn set_min_reports(env: &Env, min_reports: u32) {
    crate::ttl::set_persistent(env, &DataKey::MinReports, &min_reports);
}

pub fn get_max_staleness(env: &Env) -> u64 {
    crate::ttl::get_persistent(env, &DataKey::MaxStaleness).unwrap_or(0)
}

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
pub fn fresh_report_count(env: &Env, symbol: &soroban_sdk::Symbol) -> u32 {
    let max_staleness = get_max_staleness(env);
    if max_staleness == 0 {
        return 0;
    }

    let now = env.ledger().timestamp();
    let mut count: u32 = 0;

    for feeder in feeder_list(env).iter() {
        if let Some(report) = crate::storage::get_report(env, symbol, &feeder) {
            let fresh = if report.timestamp > now {
                true
            } else {
                now.saturating_sub(report.timestamp) <= max_staleness
            };
            if fresh {
                count += 1;
            }
        }
    }

    count
}