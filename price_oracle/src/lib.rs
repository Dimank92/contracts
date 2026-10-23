#![no_std]

//! Zenith Price Oracle — on-chain price feed for the options_market contract.
//!
//! options_market currently trusts a bare `oracle: Address` with no on-chain
//! logic behind it at all — this contract is that logic: a small set of
//! admin-authorized feeders report prices per symbol, and the aggregate
//! (median across fresh reports) is what callers read.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, vec, Address, BytesN, Env, IntoVal, Symbol, Vec,
};

#[cfg(test)]
mod storage_test;
#[cfg(test)]
mod test;

mod error;
mod events;
mod math;
mod migrations;
mod storage;
mod ttl;
mod types;

use error::Error;
use math::{median, MAX_FEEDERS};
use types::DataKey;

const DEFAULT_MAX_STALENESS: u64 = 3600; // 1 hour
const DEFAULT_MIN_REPORTS: u32 = 1; // any single fresh report is enough, the original behavior

fn require_not_paused(env: &Env) {
    let paused: bool = env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false);
    if paused {
        panic_with_error!(env, Error::ContractPaused);
    }
}

fn add_feeder_inner(env: &Env, feeder: Address) {
    migrations::require_current(env);
    if storage::is_feeder(env, &feeder) {
        panic_with_error!(env, Error::FeederAlreadyAdded);
    }
    if storage::feeder_list(env).len() >= MAX_FEEDERS {
        panic_with_error!(env, Error::TooManyFeeders);
    }
    storage::add_feeder(env, &feeder);
    events::feeder_added(env, feeder);
}

fn remove_feeder_inner(env: &Env, feeder: Address) {
    if !storage::is_feeder(env, &feeder) {
        panic_with_error!(env, Error::FeederNotFound);
    }
    storage::remove_feeder(env, &feeder);
    events::feeder_removed(env, feeder);
}

/// Cross-calls `multisig.is_approved(action_id)` via `invoke_contract`, so
/// this crate needs no multisig wasm to compile.
fn multisig_approved(env: &Env, multisig_contract: &Address, action_id: &BytesN<32>) -> bool {
    env.invoke_contract(
        multisig_contract,
        &Symbol::new(env, "is_approved"),
        vec![env, action_id.into_val(env)],
    )
}

#[contract]
pub struct PriceOracle;

#[contractimpl]
impl PriceOracle {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs. Anyone may pay the rent to keep long-lived entries alive.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    pub fn initialize(env: Env, admin: Address) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &DEFAULT_MAX_STALENESS);
        env.storage()
            .instance()
            .set(&DataKey::MinReports, &DEFAULT_MIN_REPORTS);
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    /// Admin adjusts how old a feeder's report can be and still count
    /// toward get_price's aggregate.
    pub fn set_max_staleness(env: Env, seconds: u64) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        if seconds == 0 {
            panic_with_error!(&env, Error::InvalidStaleness);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &seconds);
        events::max_staleness_updated(&env, seconds);
    }

    /// Permissionless alternative to set_max_staleness: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the admin's own signature. Worth gating the same way as
    /// pause/transfer_admin — a compromised admin widening max_staleness
    /// is a quiet way to make get_price accept increasingly stale, and
    /// therefore increasingly manipulable, prices.
    pub fn set_max_staleness_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        seconds: u64,
    ) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if seconds == 0 {
            panic_with_error!(&env, Error::InvalidStaleness);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &seconds);
        events::max_staleness_updated(&env, seconds);
    }

    pub fn get_max_staleness(env: Env) -> u64 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::MaxStaleness)
            .unwrap()
    }

    /// Admin adjusts how many CURRENTLY-fresh feeder reports get_price
    /// requires before it returns an aggregate at all. Without this,
    /// get_price will happily return a "median" backed by a single fresh
    /// report the moment every other feeder's report goes stale (or is
    /// removed) — that one feeder then fully determines the price with
    /// no averaging effect at all, defeating the point of aggregating
    /// across multiple feeders in the first place.
    pub fn set_min_reports(env: Env, count: u32) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        if count == 0 {
            panic_with_error!(&env, Error::InvalidMinReports);
        }
        env.storage().instance().set(&DataKey::MinReports, &count);
        events::min_reports_updated(&env, count);
    }

    /// Permissionless alternative to set_min_reports: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the admin's own signature. Worth gating the same way as
    /// set_max_staleness_via_multisig — a compromised admin lowering
    /// min_reports back to 1 is a quiet way to make get_price trust a
    /// single feeder again.
    pub fn set_min_reports_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        count: u32,
    ) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if count == 0 {
            panic_with_error!(&env, Error::InvalidMinReports);
        }
        env.storage().instance().set(&DataKey::MinReports, &count);
        events::min_reports_updated(&env, count);
    }

    pub fn get_min_reports(env: Env) -> u32 {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::MinReports).unwrap()
    }

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    /// Admin hands off control to a new address. Requires the CURRENT
    /// admin's signature, not the incoming one.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Permissionless alternative to transfer_admin: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the current admin's own signature — same pattern as
    /// options_market's transfer_admin_via_multisig.
    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        new_admin: Address,
    ) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Emergency stop: blocks add_feeder, remove_feeder, and report_price.
    /// Does NOT block get_price or any other view — a pause should freeze
    /// further changes to the feed, not hide the last-known price from
    /// callers still reading it.
    pub fn pause(env: Env) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    /// Permissionless alternative to pause(), same rationale and pattern
    /// as options_market's pause_via_multisig.
    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    /// Admin authorizes a new price feeder. Feeders are the only addresses
    /// allowed to call report_price.
    pub fn add_feeder(env: Env, feeder: Address) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        add_feeder_inner(&env, feeder);
    }

    /// Permissionless alternative to add_feeder: cross-calls a deployed
    /// Multisig and checks is_approved(action_id) instead of requiring
    /// the admin's own signature. Adding a feeder expands who can move
    /// get_price's aggregate, so gating it behind M-of-N is at least as
    /// warranted as pause.
    pub fn add_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        feeder: Address,
    ) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }

        add_feeder_inner(&env, feeder);
    }

    /// Admin revokes a feeder's authorization. Their most recent price
    /// report is left in storage (for audit purposes) but is no longer
    /// counted toward the aggregate once removed.
    pub fn remove_feeder(env: Env, feeder: Address) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        remove_feeder_inner(&env, feeder);
    }

    /// Permissionless alternative to remove_feeder: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the admin's own signature. Same rationale as
    /// add_feeder_via_multisig.
    pub fn remove_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        feeder: Address,
    ) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }

        remove_feeder_inner(&env, feeder);
    }

    pub fn is_feeder(env: Env, address: Address) -> bool {
        ttl::extend_instance(&env);
        storage::is_feeder(&env, &address)
    }

    pub fn get_feeder_count(env: Env) -> u32 {
        ttl::extend_instance(&env);
        storage::feeder_list(&env).len()
    }

    /// A feeder reports the current price for `symbol`. Only records this
    /// feeder's own report — see the aggregation step (still to come) for
    /// how per-feeder reports become the single price callers read.
    pub fn report_price(env: Env, feeder: Address, symbol: Symbol, price: i128) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        migrations::require_current(&env);
        feeder.require_auth();

        if !storage::is_feeder(&env, &feeder) {
            panic_with_error!(&env, Error::NotAFeeder);
        }
        if price <= 0 {
            panic_with_error!(&env, Error::InvalidPrice);
        }

        let now = env.ledger().timestamp();
        ttl::set_persistent(
            &env,
            &DataKey::PriceReport(symbol.clone(), feeder.clone()),
            &(price, now),
        );
        events::price_reported(&env, feeder, symbol, price);
    }

    pub fn get_latest_report(env: Env, symbol: Symbol, feeder: Address) -> Option<(i128, u64)> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::PriceReport(symbol, feeder))
    }

    /// The aggregate price for `symbol`: the median across every
    /// CURRENTLY authorized feeder's report that isn't older than
    /// max_staleness. Returns None if no feeder has a fresh report —
    /// callers must not treat that the same as "price is zero."
    pub fn get_price(env: Env, symbol: Symbol) -> Option<i128> {
        ttl::extend_instance(&env);
        let feeders = storage::feeder_list(&env);
        let max_staleness: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MaxStaleness)
            .unwrap();
        let now = env.ledger().timestamp();

        let mut buffer = [0i128; MAX_FEEDERS as usize];
        let mut count = 0usize;
        for feeder in feeders.iter() {
            let report: Option<(i128, u64)> =
                ttl::get_persistent(&env, &DataKey::PriceReport(symbol.clone(), feeder));
            if let Some((price, reported_at)) = report {
                if now.checked_sub(reported_at).unwrap_or(u64::MAX) <= max_staleness {
                    buffer[count] = price;
                    count += 1;
                }
            }
        }

        let min_reports: u32 = env.storage().instance().get(&DataKey::MinReports).unwrap();
        if count == 0 || (count as u32) < min_reports {
            None
        } else {
            Some(median(&mut buffer, count))
        }
    }
}
