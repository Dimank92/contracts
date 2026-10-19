#![no_std]

//! Zenith Params — a single, timelock-controlled registry for protocol
//! parameters (fee bps, settlement window, staleness, min_reports, ...).
//!
//! Every parameter carries hard `[min, max]` bounds. The timelock can
//! move a value anywhere inside its bounds with `set_param`, but
//! changing the bounds themselves goes through a slower two-step path
//! (`propose_bounds` → wait `BOUNDS_CHANGE_DELAY` → `execute_bounds`),
//! so widening the sanity envelope always takes longer than using it.
//!
//! Consumers cache values locally alongside `get_version()` and only
//! re-read parameters when the version moves (see options_market).

use soroban_sdk::{contract, contractimpl, Address, Env, Symbol, Vec};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_events;

mod error;
mod events;
mod migrations;
mod ttl;
mod types;

pub use error::Error;
pub use events::{
    BoundsCancelledEvent, BoundsProposedEvent, BoundsUpdatedEvent, InitializedEvent,
    ParamDefinedEvent, ParamUpdatedEvent,
};
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::DataKey;
pub use types::{BoundsProposal, Param};

zenith_common::contract_meta!();

/// Extra delay on top of the timelock's own delay before a bounds
/// change can take effect.
pub const BOUNDS_CHANGE_DELAY: u64 = 7 * 86_400;

#[contract]
pub struct Params;

#[contractimpl]
impl Params {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs. Anyone may pay the rent to keep long-lived entries alive.
    pub fn bump(env: Env, keys: Vec<DataKey>) -> Result<(), Error> {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
        Ok(())
    }

    pub fn initialize(env: Env, timelock: Address) -> Result<(), Error> {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Timelock) {
            return Err(Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Timelock, &timelock);
        env.storage().instance().set(&DataKey::Version, &0u64);
        zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION);
        events::initialized(&env, timelock, CURRENT_SCHEMA_VERSION);
        Ok(())
    }

    /// Creates a parameter that has never been set. Timelock only.
    pub fn define_param(
        env: Env,
        key: Symbol,
        value: i128,
        min: i128,
        max: i128,
    ) -> Result<(), Error> {
        ttl::extend_instance(&env);
        Self::require_timelock(&env)?;
        migrations::require_current(&env)?;
        let param_key = DataKey::Param(key.clone());
        if env.storage().persistent().has(&param_key) {
            return Err(Error::ParamAlreadyDefined);
        }
        if min > max {
            return Err(Error::InvalidBounds);
        }
        if value < min || value > max {
            return Err(Error::OutOfBounds);
        }
        let param = Param {
            value,
            min,
            max,
            updated_at: env.ledger().timestamp(),
        };
        ttl::set_persistent(&env, &param_key, &param);
        let version = Self::bump_version(&env)?;
        events::param_defined(
            &env,
            key,
            ParamDefinedEvent {
                value,
                min,
                max,
                updated_at: param.updated_at,
                version,
            },
        );
        Ok(())
    }

    /// Moves a parameter's value within its existing bounds. Timelock only.
    pub fn set_param(env: Env, key: Symbol, value: i128) -> Result<(), Error> {
        ttl::extend_instance(&env);
        Self::require_timelock(&env)?;
        migrations::require_current(&env)?;
        let param_key = DataKey::Param(key.clone());
        let mut param = Self::load(&env, &key)?;
        if value < param.min || value > param.max {
            return Err(Error::OutOfBounds);
        }
        let old = param.value;
        param.value = value;
        param.updated_at = env.ledger().timestamp();
        ttl::set_persistent(&env, &param_key, &param);
        let version = Self::bump_version(&env)?;
        events::param_updated(
            &env,
            key,
            ParamUpdatedEvent {
                old_value: old,
                new_value: value,
                updated_at: param.updated_at,
                version,
            },
        );
        Ok(())
    }

    /// Queues new bounds for `key`, executable after `BOUNDS_CHANGE_DELAY`.
    /// The current value must still fit inside the new bounds. Replaces
    /// any earlier pending proposal for the same key. Timelock only.
    pub fn propose_bounds(env: Env, key: Symbol, min: i128, max: i128) -> Result<(), Error> {
        ttl::extend_instance(&env);
        Self::require_timelock(&env)?;
        migrations::require_current(&env)?;
        let param = Self::load(&env, &key)?;
        if min > max {
            return Err(Error::InvalidBounds);
        }
        if param.value < min || param.value > max {
            return Err(Error::OutOfBounds);
        }
        let eta = env
            .ledger()
            .timestamp()
            .checked_add(BOUNDS_CHANGE_DELAY)
            .ok_or(Error::MathOverflow)?;
        ttl::set_persistent(
            &env,
            &DataKey::PendingBounds(key.clone()),
            &BoundsProposal { min, max, eta },
        );
        events::bounds_proposed(&env, key, BoundsProposedEvent { min, max, eta });
        Ok(())
    }

    /// Applies a queued bounds change once its delay has elapsed. Timelock only.
    pub fn execute_bounds(env: Env, key: Symbol) -> Result<(), Error> {
        ttl::extend_instance(&env);
        Self::require_timelock(&env)?;
        migrations::require_current(&env)?;
        let pending_key = DataKey::PendingBounds(key.clone());
        let proposal: BoundsProposal =
            ttl::get_persistent(&env, &pending_key).ok_or(Error::NoPendingBounds)?;
        if env.ledger().timestamp() < proposal.eta {
            return Err(Error::BoundsDelayNotElapsed);
        }
        let mut param = Self::load(&env, &key)?;
        // set_param can't have moved the value outside the new bounds
        // unless they're narrower than the old ones — re-check.
        if param.value < proposal.min || param.value > proposal.max {
            return Err(Error::OutOfBounds);
        }
        param.min = proposal.min;
        param.max = proposal.max;
        param.updated_at = env.ledger().timestamp();
        ttl::set_persistent(&env, &DataKey::Param(key.clone()), &param);
        env.storage().persistent().remove(&pending_key);
        let version = Self::bump_version(&env)?;
        events::bounds_updated(
            &env,
            key,
            BoundsUpdatedEvent {
                min: proposal.min,
                max: proposal.max,
                updated_at: param.updated_at,
                version,
            },
        );
        Ok(())
    }

    pub fn cancel_bounds(env: Env, key: Symbol) -> Result<(), Error> {
        ttl::extend_instance(&env);
        Self::require_timelock(&env)?;
        migrations::require_current(&env)?;
        let pending_key = DataKey::PendingBounds(key.clone());
        let proposal: BoundsProposal =
            ttl::get_persistent(&env, &pending_key).ok_or(Error::NoPendingBounds)?;
        env.storage().persistent().remove(&pending_key);
        events::bounds_cancelled(
            &env,
            key,
            BoundsCancelledEvent {
                min: proposal.min,
                max: proposal.max,
                eta: proposal.eta,
            },
        );
        Ok(())
    }

    pub fn get_param(env: Env, key: Symbol) -> Result<Option<Param>, Error> {
        ttl::extend_instance(&env);
        Ok(ttl::get_persistent(&env, &DataKey::Param(key)))
    }

    pub fn get_value(env: Env, key: Symbol) -> Result<i128, Error> {
        ttl::extend_instance(&env);
        Ok(Self::load(&env, &key)?.value)
    }

    pub fn get_pending_bounds(env: Env, key: Symbol) -> Result<Option<BoundsProposal>, Error> {
        ttl::extend_instance(&env);
        Ok(ttl::get_persistent(&env, &DataKey::PendingBounds(key)))
    }

    pub fn get_version(env: Env) -> Result<u64, Error> {
        ttl::extend_instance(&env);
        Ok(env.storage().instance().get(&DataKey::Version).unwrap_or(0))
    }

    pub fn get_timelock(env: Env) -> Result<Address, Error> {
        ttl::extend_instance(&env);
        Self::timelock(&env)
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Timelock only. Returns
    /// the version reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, from: u32, to: u32) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        Self::require_timelock(&env)?;
        zenith_common::migrations::migrate(
            &env,
            from,
            to,
            CURRENT_SCHEMA_VERSION,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by the authorized `migrate` call and the
    /// steps are code in this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    pub fn schema_version(env: Env) -> Result<u32, Error> {
        ttl::extend_instance(&env);
        Ok(zenith_common::migrations::schema_version(&env))
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Result<Option<(u32, u32)>, Error> {
        ttl::extend_instance(&env);
        Ok(zenith_common::migrations::migration_state(&env))
    }
}

impl Params {
    fn timelock(env: &Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&DataKey::Timelock)
            .ok_or(Error::NotInitialized)
    }

    fn require_timelock(env: &Env) -> Result<(), Error> {
        Self::timelock(env)?.require_auth();
        Ok(())
    }

    fn load(env: &Env, key: &Symbol) -> Result<Param, Error> {
        ttl::get_persistent(env, &DataKey::Param(key.clone())).ok_or(Error::ParamNotFound)
    }

    /// Bumps and returns the registry version.
    fn bump_version(env: &Env) -> Result<u64, Error> {
        let version: u64 = env.storage().instance().get(&DataKey::Version).unwrap_or(0);
        let next = version.checked_add(1).ok_or(Error::MathOverflow)?;
        env.storage().instance().set(&DataKey::Version, &next);
        Ok(next)
    }
}
