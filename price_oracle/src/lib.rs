#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, Symbol, Vec,
};

use common::{require_admin_or_multisig, ActionClass, Auth};

mod feeder_staking;
mod math;
mod oracle_adapter;
mod outlier_filter;
mod round_history;
mod storage;

#[cfg(test)]
mod test;

/// Default minimum number of fresh feeder reports required to produce an aggregate.
pub const DEFAULT_MIN_REPORTS: u32 = 1;

/// Default staleness window (in seconds) used when none is configured.
pub const DEFAULT_MAX_STALENESS: u64 = 300;

/// Storage keys used by the price_oracle contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Contract admin address.
    Admin,
    /// Whether the contract is paused.
    Paused,
    /// Minimum number of fresh feeder reports required for an aggregate.
    MinReports,
    /// Staleness window in seconds.
    MaxStaleness,
    /// A single feeder report keyed by feeder address.
    Report(Address),
    /// List of registered feeder addresses.
    Feeders,
}

/// Typed errors returned by the price_oracle contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    /// Contract has not been initialized.
    NotInitialized = 1,
    /// Caller is not authorized for this action.
    Unauthorized = 2,
    /// Contract is paused.
    Paused = 3,
    /// `min_reports` must be greater than zero.
    InvalidMinReports = 4,
    /// Staleness window must be greater than zero and satisfiable.
    InvalidStaleness = 5,
    /// No aggregate could be produced (quorum or freshness not met).
    NoAggregate = 6,
}

/// A single feeder price report.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    /// Reported price.
    pub price: i128,
    /// Ledger timestamp at which the report was submitted.
    pub timestamp: u64,
}

/// Action classes used for admin/multisig authorization.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PriceOracleAction {
    /// Setting the minimum report quorum.
    SetMinReports,
    /// Setting the staleness window.
    SetMaxStaleness,
}

impl PriceOracleAction {
    /// Returns the dedicated `ActionClass` for this action.
    pub fn class(self) -> ActionClass {
        match self {
            PriceOracleAction::SetMinReports => ActionClass::PriceOracleSetMinReports,
            PriceOracleAction::SetMaxStaleness => ActionClass::PriceOracleSetMaxStaleness,
        }
    }
}

#[contract]
pub struct PriceOracle;

#[contractimpl]
impl PriceOracle {
    /// Initializes the contract with an admin and default policy values.
    ///
    /// Sets `Admin`, `Paused = false`, `MaxStaleness = DEFAULT_MAX_STALENESS`,
    /// and `MinReports = DEFAULT_MIN_REPORTS`.
    pub fn initialize(env: Env, admin: Address) {
        if storage::has_admin(&env) {
            // Already initialized; do not overwrite.
            return;
        }
        storage::set_admin(&env, &admin);
        storage::set_paused(&env, false);
        storage::set_min_reports(&env, DEFAULT_MIN_REPORTS);
        storage::set_max_staleness(&env, DEFAULT_MAX_STALENESS);
        storage::extend_instance_ttl(&env);
    }

    /// Sets the minimum number of fresh feeder reports required for an aggregate.
    ///
    /// Requires admin or multisig authorization. `min_reports` must be greater
    /// than zero. Note that if `min_reports` exceeds the number of registered
    /// feeders, no aggregate can ever be produced until more feeders register
    /// or the value is lowered; aggregation fails closed.
    pub fn set_min_reports(env: Env, auth: Auth, min_reports: u32) -> Result<(), Error> {
        Self::set_min_reports_inner(&env, auth, min_reports)
    }

    /// Multisig variant of [`set_min_reports`].
    pub fn set_min_reports_via_multisig(
        env: Env,
        auth: Auth,
        min_reports: u32,
    ) -> Result<(), Error> {
        Self::set_min_reports_inner(&env, auth, min_reports)
    }

    fn set_min_reports_inner(env: &Env, auth: Auth, min_reports: u32) -> Result<(), Error> {
        Self::require_initialized(env)?;
        require_admin_or_multisig(env, &auth, PriceOracleAction::SetMinReports.class());

        if min_reports == 0 {
            return Err(Error::InvalidMinReports);
        }

        storage::set_min_reports(env, min_reports);
        storage::extend_instance_ttl(env);

        env.events().publish(
            (symbol_short!("min_rpt"), symbol_short!("updated")),
            (min_reports, 1u32),
        );

        Ok(())
    }

    /// Sets the staleness window (in seconds) used to decide which reports are fresh.
    ///
    /// Requires admin or multisig authorization. The window must be greater
    /// than zero. When the window elapses for every feeder, aggregation fails
    /// closed and returns `None` rather than falling back to a stale price.
    pub fn set_max_staleness(env: Env, auth: Auth, max_staleness: u64) -> Result<(), Error> {
        Self::set_max_staleness_inner(&env, auth, max_staleness)
    }

    /// Multisig variant of [`set_max_staleness`].
    pub fn set_max_staleness_via_multisig(
        env: Env,
        auth: Auth,
        max_staleness: u64,
    ) -> Result<(), Error> {
        Self::set_max_staleness_inner(&env, auth, max_staleness)
    }

    fn set_max_staleness_inner(env: &Env, auth: Auth, max_staleness: u64) -> Result<(), Error> {
        Self::require_initialized(env)?;
        require_admin_or_multisig(env, &auth, PriceOracleAction::SetMaxStaleness.class());

        if max_staleness == 0 {
            return Err(Error::InvalidStaleness);
        }

        storage::set_max_staleness(env, max_staleness);
        storage::extend_instance_ttl(env);

        env.events().publish(
            (symbol_short!("max_stal"), symbol_short!("updated")),
            (max_staleness, 1u32),
        );

        Ok(())
    }

    /// Returns the current minimum report quorum. Unauthenticated.
    pub fn get_min_reports(env: Env) -> u32 {
        storage::get_min_reports(&env)
    }

    /// Returns the current staleness window in seconds. Unauthenticated.
    pub fn get_max_staleness(env: Env) -> u64 {
        storage::get_max_staleness(&env)
    }

    /// Returns the current admin address, if any.
    pub fn get_admin(env: Env) -> Option<Address> {
        storage::get_admin(&env)
    }

    /// Returns whether the contract is paused.
    pub fn is_paused(env: Env) -> bool {
        storage::is_paused(&env)
    }

    /// Computes the aggregate price across fresh feeder reports.
    ///
    /// Returns `None` (fails closed) when:
    /// - the contract is paused,
    /// - fewer than `min_reports` fresh reports exist within the staleness window.
    ///
    /// Freshness is determined using `env.ledger().timestamp()`.
    pub fn get_price(env: Env) -> Option<i128> {
        if storage::is_paused(&env) {
            return None;
        }

        let min_reports = storage::get_min_reports(&env);
        let max_staleness = storage::get_max_staleness(&env);
        let now = env.ledger().timestamp();

        let feeders: Vec<Address> = storage::get_feeders(&env);
        let mut fresh: Vec<i128> = Vec::new(&env);

        for feeder in feeders.iter() {
            if let Some(report) = storage::get_report(&env, &feeder) {
                let age = now.saturating_sub(report.timestamp);
                if age <= max_staleness {
                    fresh.push_back(report.price);
                }
            }
        }

        if (fresh.len() as u32) < min_reports {
            return None;
        }

        Some(outlier_filter::median(&fresh))
    }

    fn require_initialized(env: &Env) -> Result<(), Error> {
        if !storage::has_admin(env) {
            return Err(Error::NotInitialized);
        }
        Ok(())
    }
}