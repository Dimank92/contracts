#![no_std]

//! Zenith Price Oracle — on-chain price feed for the options_market contract.
//!
//! A small set of admin-authorized feeders report prices per symbol, and
//! the aggregate (median across fresh reports, gated by a `min_reports`
//! quorum) is what callers read. Reports enter and the aggregate leaves
//! only through `oracle_adapter`, so `get_price` — the call
//! `options_market::set_settlement_price_from_oracle` makes — and any
//! admin path reading the same deployment see one price type and one
//! validation rule. Numeric primitives live in `math`.
//!
//! Every admin function has a `_via_multisig` twin that trusts only the
//! multisig pinned with `set_multisig` (never a caller-supplied address),
//! and consumes the approval it used.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, Address, BytesN, Env, IntoVal, Symbol, Vec,
};

#[cfg(test)]
mod storage_test;
#[cfg(test)]
mod test;
#[cfg(test)]
mod test_adversaries;

mod error;
mod events;
pub mod math;
mod migrations;
pub mod oracle_adapter;
// MAD-based robust aggregation; not yet the get_price policy, but compiled
// (and its property tests run) so it can't silently rot.
#[allow(dead_code)]
mod outlier_filter;
mod storage;
mod ttl;
pub mod types;

pub use error::Error;
pub use oracle_adapter::{FeederReport, OraclePrice};

use math::{MAX_FEEDERS, PRICE_DECIMALS};
use types::DataKey;

const DEFAULT_MAX_STALENESS: u64 = 3600; // 1 hour
const DEFAULT_MIN_REPORTS: u32 = 1; // any single fresh report is enough, the original behavior

/// Who is authorizing an admin action.
enum Auth {
    /// The stored admin signs.
    Admin,
    /// `(multisig_contract, action_id)`: must be the pinned multisig, and
    /// it must report `action_id` approved.
    Multisig(Address, BytesN<32>),
}

fn admin(env: &Env) -> Address {
    zenith_common::get_admin(env, &DataKey::Admin, Error::InternalInvariant).unwrap_or_else(|e| fail(env, e))
}

fn fail(env: &Env, err: Error) -> ! {
    panic_with_error!(env, err)
}

/// Panics with `Unauthorized` unless `auth` holds. A multisig approval is
/// consumed (`reset`) before the action runs, so one approval authorizes
/// exactly one call. Any failure talking to the multisig — wrong
/// interface, revert — counts as "not approved".
fn require_auth(env: &Env, auth: &Auth) {
    match auth {
        Auth::Admin => admin(env).require_auth(),
        Auth::Multisig(multisig, action_id) => {
            let trusted: Option<Address> = env.storage().instance().get(&DataKey::TrustedMultisig);
            if trusted.as_ref() != Some(multisig) {
                fail(env, Error::Unauthorized);
            }
            let approved = env.try_invoke_contract::<bool, soroban_sdk::Error>(
                multisig,
                &Symbol::new(env, "is_approved"),
                soroban_sdk::vec![env, action_id.into_val(env)],
            );
            if !matches!(approved, Ok(Ok(true))) {
                fail(env, Error::Unauthorized);
            }
            env.invoke_contract::<()>(
                multisig,
                &Symbol::new(env, "reset"),
                soroban_sdk::vec![env, action_id.into_val(env)],
            );
        }
    }
}

fn require_not_paused(env: &Env) {
    if let Err(e) = zenith_common::require_not_paused(env, &DataKey::Paused, Error::ContractPaused) {
        fail(env, e);
    }
}

fn max_staleness(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&DataKey::MaxStaleness)
        .unwrap_or(DEFAULT_MAX_STALENESS)
}

fn min_reports(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::MinReports)
        .unwrap_or(DEFAULT_MIN_REPORTS)
}

fn set_max_staleness_inner(env: &Env, auth: Auth, seconds: u64) {
    migrations::require_current(env);
    require_auth(env, &auth);
    if seconds == 0 {
        fail(env, Error::InvalidStaleness);
    }
    env.storage()
        .instance()
        .set(&DataKey::MaxStaleness, &seconds);
    events::max_staleness_updated(env, seconds);
}

fn set_min_reports_inner(env: &Env, auth: Auth, count: u32) {
    migrations::require_current(env);
    require_auth(env, &auth);
    if count == 0 || count > MAX_FEEDERS {
        fail(env, Error::InvalidMinReports);
    }
    env.storage().instance().set(&DataKey::MinReports, &count);
    events::min_reports_updated(env, count);
}

fn transfer_admin_inner(env: &Env, auth: Auth, new_admin: Address) {
    migrations::require_current(env);
    require_auth(env, &auth);
    let old = zenith_common::set_admin(env, &DataKey::Admin, &new_admin, Error::InternalInvariant).unwrap_or_else(|e| fail(env, e));
    events::admin_transferred(env, old, new_admin);
}

fn set_paused_inner(env: &Env, auth: Auth, paused: bool) {
    require_auth(env, &auth);
    zenith_common::set_paused(env, &DataKey::Paused, paused);
    if paused {
        events::paused(env);
    } else {
        events::unpaused(env);
    }
}

fn add_feeder_inner(env: &Env, auth: Auth, feeder: Address) {
    migrations::require_current(env);
    require_not_paused(env);
    require_auth(env, &auth);
    if storage::is_feeder(env, &feeder) {
        fail(env, Error::FeederAlreadyAdded);
    }
    if storage::feeder_list(env).len() >= MAX_FEEDERS {
        fail(env, Error::TooManyFeeders);
    }
    storage::add_feeder(env, &feeder);
    events::feeder_added(env, feeder);
}

fn remove_feeder_inner(env: &Env, auth: Auth, feeder: Address) {
    migrations::require_current(env);
    require_not_paused(env);
    require_auth(env, &auth);
    if !storage::is_feeder(env, &feeder) {
        fail(env, Error::FeederNotFound);
    }
    storage::remove_feeder(env, &feeder);
    events::feeder_removed(env, feeder);
}

fn rotate_feeder_inner(env: &Env, auth: Auth, old_feeder: Address, new_feeder: Address) {
    migrations::require_current(env);
    require_not_paused(env);
    require_auth(env, &auth);
    if old_feeder == new_feeder {
        fail(env, Error::FeederAlreadyAdded);
    }
    if !storage::is_feeder(env, &old_feeder) {
        fail(env, Error::FeederNotFound);
    }
    if storage::is_feeder(env, &new_feeder) {
        fail(env, Error::FeederAlreadyAdded);
    }
    storage::replace_feeder(env, &old_feeder, &new_feeder);
    events::feeder_rotated(env, old_feeder, new_feeder);
}

fn add_feeder_for_symbol_inner(env: &Env, auth: Auth, symbol: Symbol, feeder: Address) {
    migrations::require_current(env);
    require_not_paused(env);
    require_auth(env, &auth);
    if oracle_adapter::symbol_precision(env, &symbol).is_none() {
        fail(env, Error::UnknownSymbol);
    }
    if storage::is_feeder_for_symbol(env, &symbol, &feeder) {
        fail(env, Error::FeederAlreadyAdded);
    }
    if storage::symbol_feeder_list(env, &symbol).len() >= MAX_FEEDERS {
        fail(env, Error::TooManyFeeders);
    }
    storage::add_symbol_feeder(env, &symbol, &feeder);
    events::symbol_feeder_added(env, symbol, feeder);
}

fn remove_feeder_for_symbol_inner(env: &Env, auth: Auth, symbol: Symbol, feeder: Address) {
    migrations::require_current(env);
    require_not_paused(env);
    require_auth(env, &auth);
    if oracle_adapter::symbol_precision(env, &symbol).is_none() {
        fail(env, Error::UnknownSymbol);
    }
    if !storage::is_feeder_for_symbol(env, &symbol, &feeder) {
        fail(env, Error::FeederNotFound);
    }
    if storage::symbol_feeder_list(env, &symbol).len() <= min_reports(env) {
        fail(env, Error::BelowQuorum);
    }
    storage::remove_symbol_feeder(env, &symbol, &feeder);
    events::symbol_feeder_removed(env, symbol, feeder);
}

fn rotate_feeder_for_symbol_inner(
    env: &Env,
    auth: Auth,
    symbol: Symbol,
    old_feeder: Address,
    new_feeder: Address,
) {
    migrations::require_current(env);
    require_not_paused(env);
    require_auth(env, &auth);
    if oracle_adapter::symbol_precision(env, &symbol).is_none() {
        fail(env, Error::UnknownSymbol);
    }
    if old_feeder == new_feeder {
        fail(env, Error::FeederAlreadyAdded);
    }
    if !storage::is_feeder_for_symbol(env, &symbol, &old_feeder) {
        fail(env, Error::FeederNotFound);
    }
    if storage::is_feeder_for_symbol(env, &symbol, &new_feeder) {
        fail(env, Error::FeederAlreadyAdded);
    }
    storage::replace_symbol_feeder(env, &symbol, &old_feeder, &new_feeder);
    events::symbol_feeder_rotated(env, symbol, old_feeder, new_feeder);
}

fn report_inner(env: &Env, report: FeederReport) {
    migrations::require_current(env);
    require_not_paused(env);
    report.feeder.require_auth();
    if !storage::is_effective_feeder(env, &report.symbol, &report.feeder) {
        fail(env, Error::NotAFeeder);
    }
    if let Err(e) = oracle_adapter::ingest(env, report, max_staleness(env)) {
        fail(env, e);
    }
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

    /// One-time setup. Requires `admin`'s signature. Seeds XLM, BTC, ETH
    /// and SOL as known symbols, a 1-hour `max_staleness` and a
    /// `min_reports` of 1.
    pub fn initialize(env: Env, admin: Address) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            fail(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        let instance = env.storage().instance();
        instance.set(&DataKey::Admin, &admin);
        instance.set(&DataKey::MaxStaleness, &DEFAULT_MAX_STALENESS);
        instance.set(&DataKey::MinReports, &DEFAULT_MIN_REPORTS);
        oracle_adapter::seed_default_symbols(&env);
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    // ── multisig ────────────────────────────────────────────────────────────

    /// Pins the one multisig every `*_via_multisig` entrypoint trusts.
    /// Admin only; re-pinning replaces it. Until it's set, those
    /// entrypoints reject every call.
    pub fn set_multisig(env: Env, multisig: Address) {
        ttl::extend_instance(&env);
        migrations::require_current(&env);
        require_auth(&env, &Auth::Admin);
        env.storage()
            .instance()
            .set(&DataKey::TrustedMultisig, &multisig);
        events::multisig_set(&env, multisig);
    }

    pub fn get_multisig(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::TrustedMultisig)
    }

    // ── staleness / quorum ──────────────────────────────────────────────────

    /// Admin adjusts how old a feeder's report can be and still count
    /// toward get_price's aggregate.
    pub fn set_max_staleness(env: Env, seconds: u64) {
        ttl::extend_instance(&env);
        set_max_staleness_inner(&env, Auth::Admin, seconds);
    }

    /// Multisig twin of `set_max_staleness` — a compromised admin
    /// widening max_staleness is a quiet way to make get_price accept
    /// increasingly stale, and therefore manipulable, prices.
    pub fn set_max_staleness_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        seconds: u64,
    ) {
        ttl::extend_instance(&env);
        set_max_staleness_inner(&env, Auth::Multisig(multisig_contract, action_id), seconds);
    }

    pub fn get_max_staleness(env: Env) -> u64 {
        ttl::extend_instance(&env);
        max_staleness(&env)
    }

    /// Admin sets how many CURRENTLY-fresh feeder reports get_price
    /// requires (1..=MAX_FEEDERS) before it returns an aggregate at all,
    /// so one feeder can't determine the price alone just because the
    /// others went stale or were removed.
    pub fn set_min_reports(env: Env, count: u32) {
        ttl::extend_instance(&env);
        set_min_reports_inner(&env, Auth::Admin, count);
    }

    /// Multisig twin of `set_min_reports`.
    pub fn set_min_reports_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        count: u32,
    ) {
        ttl::extend_instance(&env);
        set_min_reports_inner(&env, Auth::Multisig(multisig_contract, action_id), count);
    }

    pub fn get_min_reports(env: Env) -> u32 {
        ttl::extend_instance(&env);
        min_reports(&env)
    }

    // ── admin ───────────────────────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        admin(&env)
    }

    /// Admin hands off control to a new address. Requires the CURRENT
    /// admin's signature, not the incoming one.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        ttl::extend_instance(&env);
        transfer_admin_inner(&env, Auth::Admin, new_admin);
    }

    /// Multisig twin of `transfer_admin`.
    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        new_admin: Address,
    ) {
        ttl::extend_instance(&env);
        transfer_admin_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            new_admin,
        );
    }

    // ── pause ───────────────────────────────────────────────────────────────

    /// Emergency stop: blocks add_feeder, remove_feeder, and report_price.
    /// Does NOT block get_price or any other view — a pause freezes the
    /// feed, it doesn't hide the last-known price from readers. Not
    /// blocked by an in-progress migration, so it stays usable then.
    pub fn pause(env: Env) {
        ttl::extend_instance(&env);
        set_paused_inner(&env, Auth::Admin, true);
    }

    pub fn unpause(env: Env) {
        ttl::extend_instance(&env);
        set_paused_inner(&env, Auth::Admin, false);
    }

    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        set_paused_inner(&env, Auth::Multisig(multisig_contract, action_id), true);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        set_paused_inner(&env, Auth::Multisig(multisig_contract, action_id), false);
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        zenith_common::is_paused(&env, &DataKey::Paused)
    }

    // ── symbols ─────────────────────────────────────────────────────────────

    /// Admin declares `symbol` reportable with `precision` meaningful
    /// decimals (0..=7): reports must be whole multiples of
    /// `10^(7 - precision)` at the oracle's 7-decimal scale.
    pub fn set_symbol_precision(env: Env, symbol: Symbol, precision: u32) {
        ttl::extend_instance(&env);
        migrations::require_current(&env);
        require_auth(&env, &Auth::Admin);
        if let Err(e) = oracle_adapter::set_symbol_precision(&env, &symbol, precision) {
            fail(&env, e);
        }
    }

    /// `None` for a symbol that can't be reported.
    pub fn get_symbol_precision(env: Env, symbol: Symbol) -> Option<u32> {
        ttl::extend_instance(&env);
        oracle_adapter::symbol_precision(&env, &symbol)
    }

    // ── feeders ─────────────────────────────────────────────────────────────

    /// Admin authorizes a new price feeder. Feeders are the only addresses
    /// allowed to report prices. At most MAX_FEEDERS at once.
    pub fn add_feeder(env: Env, feeder: Address) {
        ttl::extend_instance(&env);
        add_feeder_inner(&env, Auth::Admin, feeder);
    }

    /// Multisig twin of `add_feeder`: adding a feeder expands who can
    /// move the aggregate, so M-of-N gating is at least as warranted as
    /// for pause.
    pub fn add_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        feeder: Address,
    ) {
        ttl::extend_instance(&env);
        add_feeder_inner(&env, Auth::Multisig(multisig_contract, action_id), feeder);
    }

    /// Admin revokes a feeder. Their latest reports stay in storage (audit
    /// trail) but stop counting toward the aggregate immediately, even
    /// mid-round.
    pub fn remove_feeder(env: Env, feeder: Address) {
        ttl::extend_instance(&env);
        remove_feeder_inner(&env, Auth::Admin, feeder);
    }

    /// Multisig twin of `remove_feeder`.
    pub fn remove_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        feeder: Address,
    ) {
        ttl::extend_instance(&env);
        remove_feeder_inner(&env, Auth::Multisig(multisig_contract, action_id), feeder);
    }

    pub fn is_feeder(env: Env, address: Address) -> bool {
        ttl::extend_instance(&env);
        storage::is_feeder(&env, &address)
    }

    pub fn get_feeder_count(env: Env) -> u32 {
        ttl::extend_instance(&env);
        storage::feeder_list(&env).len()
    }

    pub fn get_feeders(env: Env) -> Vec<Address> {
        ttl::extend_instance(&env);
        storage::feeder_list(&env)
    }

    /// Admin rotates/replaces an existing feeder with a new address.
    pub fn rotate_feeder(env: Env, old_feeder: Address, new_feeder: Address) {
        ttl::extend_instance(&env);
        rotate_feeder_inner(&env, Auth::Admin, old_feeder, new_feeder);
    }

    /// Multisig twin of `rotate_feeder`.
    pub fn rotate_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        old_feeder: Address,
        new_feeder: Address,
    ) {
        ttl::extend_instance(&env);
        rotate_feeder_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            old_feeder,
            new_feeder,
        );
    }

    /// Admin registers a feeder authorized specifically for `symbol`.
    pub fn add_feeder_for_symbol(env: Env, symbol: Symbol, feeder: Address) {
        ttl::extend_instance(&env);
        add_feeder_for_symbol_inner(&env, Auth::Admin, symbol, feeder);
    }


    /// Admin revokes a feeder authorized for `symbol`.
    pub fn remove_feeder_for_symbol(env: Env, symbol: Symbol, feeder: Address) {
        ttl::extend_instance(&env);
        remove_feeder_for_symbol_inner(&env, Auth::Admin, symbol, feeder);
    }


    /// Admin rotates/replaces a feeder authorized specifically for `symbol`.
    pub fn rotate_feeder_for_symbol(
        env: Env,
        symbol: Symbol,
        old_feeder: Address,
        new_feeder: Address,
    ) {
        ttl::extend_instance(&env);
        rotate_feeder_for_symbol_inner(&env, Auth::Admin, symbol, old_feeder, new_feeder);
    }


    pub fn is_feeder_for_symbol(env: Env, symbol: Symbol, address: Address) -> bool {
        ttl::extend_instance(&env);
        storage::is_feeder_for_symbol(&env, &symbol, &address)
    }

    pub fn get_feeder_count_for_symbol(env: Env, symbol: Symbol) -> u32 {
        ttl::extend_instance(&env);
        storage::symbol_feeder_list(&env, &symbol).len()
    }

    pub fn get_feeders_for_symbol(env: Env, symbol: Symbol) -> Vec<Address> {
        ttl::extend_instance(&env);
        storage::symbol_feeder_list(&env, &symbol)
    }

    // ── reports ─────────────────────────────────────────────────────────────

    /// A feeder reports `symbol`'s current price at the oracle's 7-decimal
    /// scale, observed now. Validated by `oracle_adapter::validate`.
    pub fn report_price(env: Env, feeder: Address, symbol: Symbol, price: i128) {
        ttl::extend_instance(&env);
        let report = FeederReport {
            symbol,
            feeder,
            price,
            timestamp: env.ledger().timestamp(),
        };
        report_inner(&env, report);
    }

    /// Like `report_price`, for a price from an external feed quoted with
    /// `decimals` implied decimals (0..=18; normalized down to 7, rounding
    /// toward zero) and observed at `observed_at`. `observed_at` may not
    /// be in the future, older than `max_staleness`, or older than this
    /// feeder's previous report for the symbol.
    pub fn report_price_scaled(
        env: Env,
        feeder: Address,
        symbol: Symbol,
        price: i128,
        decimals: u32,
        observed_at: u64,
    ) {
        ttl::extend_instance(&env);
        let report = oracle_adapter::normalize(symbol, feeder, price, decimals, observed_at)
            .unwrap_or_else(|e| fail(&env, e));
        report_inner(&env, report);
    }

    /// `(price, timestamp)` of `feeder`'s latest report for `symbol`, kept
    /// even after the feeder is removed.
    pub fn get_latest_report(env: Env, symbol: Symbol, feeder: Address) -> Option<(i128, u64)> {
        ttl::extend_instance(&env);
        oracle_adapter::latest_report(&env, &symbol, &feeder)
    }

    // ── aggregate ───────────────────────────────────────────────────────────

    /// The aggregate price for `symbol` at 7 decimals (options_market's
    /// PRICE_PRECISION): the median across every CURRENTLY authorized
    /// feeder's report no older than max_staleness, once at least
    /// min_reports are fresh. `None` otherwise — callers must not treat
    /// that as "price is zero". Stays readable while paused.
    pub fn get_price(env: Env, symbol: Symbol) -> Option<i128> {
        ttl::extend_instance(&env);
        oracle_adapter::current_price(&env, &symbol, max_staleness(&env), min_reports(&env))
            .ok()
            .map(|p| p.price)
    }

    /// `get_price` with its provenance, or the reason there is none
    /// (`NoReports`, `StalePrice`, `BelowQuorum`).
    pub fn get_price_data(env: Env, symbol: Symbol) -> Result<OraclePrice, Error> {
        ttl::extend_instance(&env);
        oracle_adapter::current_price(&env, &symbol, max_staleness(&env), min_reports(&env))
    }

    /// Implied decimals of every price this contract returns.
    pub fn decimals(_env: Env) -> u32 {
        PRICE_DECIMALS
    }

    pub fn schema_version(env: Env) -> u32 {
        ttl::extend_instance(&env);
        zenith_common::migrations::schema_version(&env)
    }
}
