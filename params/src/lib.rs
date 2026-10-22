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

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, Symbol, Vec};

#[cfg(test)]
mod test;

mod error;
mod events;
mod migrations;
mod ttl;
mod types;

use error::Error;
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::DataKey;
pub use types::{BoundsProposal, Param, ParamValue, TypedKey, ValueType};

/// Default page size for `list_namespace` when the caller passes 0.
pub const DEFAULT_LIST_LIMIT: u32 = 20;
/// Hard cap on a single `list_namespace` page.
pub const MAX_LIST_LIMIT: u32 = 100;

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
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    pub fn initialize(env: Env, timelock: Address) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Timelock) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Timelock, &timelock);
        env.storage().instance().set(&DataKey::Version, &0u64);
        zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION);
    }

    /// Creates a parameter that has never been set. Timelock only.
    pub fn define_param(env: Env, key: Symbol, value: i128, min: i128, max: i128) {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
        migrations::require_current(&env);
        let param_key = DataKey::Param(key.clone());
        if env.storage().persistent().has(&param_key) {
            panic_with_error!(&env, Error::ParamAlreadyDefined);
        }
        if min > max {
            panic_with_error!(&env, Error::InvalidBounds);
        }
        if value < min || value > max {
            panic_with_error!(&env, Error::OutOfBounds);
        }
        let param = Param {
            value,
            min,
            max,
            updated_at: env.ledger().timestamp(),
        };
        ttl::set_persistent(&env, &param_key, &param);
        Self::bump_version(&env);
        events::param_defined(&env, key, value, min, max);
    }

    /// Moves a parameter's value within its existing bounds. Timelock only.
    pub fn set_param(env: Env, key: Symbol, value: i128) {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
        migrations::require_current(&env);
        let param_key = DataKey::Param(key.clone());
        let mut param = Self::load(&env, &key);
        if value < param.min || value > param.max {
            panic_with_error!(&env, Error::OutOfBounds);
        }
        let old = param.value;
        param.value = value;
        param.updated_at = env.ledger().timestamp();
        ttl::set_persistent(&env, &param_key, &param);
        Self::bump_version(&env);
        events::param_updated(&env, key, old, value);
    }

    /// Queues new bounds for `key`, executable after `BOUNDS_CHANGE_DELAY`.
    /// The current value must still fit inside the new bounds. Replaces
    /// any earlier pending proposal for the same key. Timelock only.
    pub fn propose_bounds(env: Env, key: Symbol, min: i128, max: i128) {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
        migrations::require_current(&env);
        let param = Self::load(&env, &key);
        if min > max {
            panic_with_error!(&env, Error::InvalidBounds);
        }
        if param.value < min || param.value > max {
            panic_with_error!(&env, Error::OutOfBounds);
        }
        let eta = env.ledger().timestamp() + BOUNDS_CHANGE_DELAY;
        ttl::set_persistent(
            &env,
            &DataKey::PendingBounds(key.clone()),
            &BoundsProposal { min, max, eta },
        );
        events::bounds_proposed(&env, key, min, max, eta);
    }

    /// Applies a queued bounds change once its delay has elapsed. Timelock only.
    pub fn execute_bounds(env: Env, key: Symbol) {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
        migrations::require_current(&env);
        let pending_key = DataKey::PendingBounds(key.clone());
        let proposal: BoundsProposal = ttl::get_persistent(&env, &pending_key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NoPendingBounds));
        if env.ledger().timestamp() < proposal.eta {
            panic_with_error!(&env, Error::BoundsDelayNotElapsed);
        }
        let mut param = Self::load(&env, &key);
        // set_param can't have moved the value outside the new bounds
        // unless they're narrower than the old ones — re-check.
        if param.value < proposal.min || param.value > proposal.max {
            panic_with_error!(&env, Error::OutOfBounds);
        }
        param.min = proposal.min;
        param.max = proposal.max;
        param.updated_at = env.ledger().timestamp();
        ttl::set_persistent(&env, &DataKey::Param(key.clone()), &param);
        env.storage().persistent().remove(&pending_key);
        Self::bump_version(&env);
        events::bounds_updated(&env, key, proposal.min, proposal.max);
    }

    pub fn cancel_bounds(env: Env, key: Symbol) {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
        migrations::require_current(&env);
        let pending_key = DataKey::PendingBounds(key.clone());
        if !env.storage().persistent().has(&pending_key) {
            panic_with_error!(&env, Error::NoPendingBounds);
        }
        env.storage().persistent().remove(&pending_key);
        events::bounds_cancelled(&env, key);
    }

    // ── Typed, namespaced registry ──────────────────────────────────────────

    /// Writes a typed value under `(namespace, key, type_tag)`. Admin or
    /// multisig only. The type tag is part of the storage key, so a `u32`
    /// and an `i64` under the same name never collide. Overwrites any
    /// existing value of the same type; emits old/new in the event.
    pub fn set_value(env: Env, namespace: Symbol, key: Symbol, value: ParamValue) {
        ttl::extend_instance(&env);
        zenith_common::require_admin_or_multisig(&env);
        migrations::require_current(&env);
        let typed = TypedKey::new(namespace.clone(), key.clone(), value.type_tag());
        let storage_key = DataKey::Typed(typed.clone());
        let old = ttl::get_persistent::<ParamValue>(&env, &storage_key);
        ttl::set_persistent(&env, &storage_key, &value);
        Self::bump_version(&env);
        events::value_set(&env, namespace, key, value.type_tag(), old, value);
    }

    /// Reads a typed value. Permissionless. Fails closed with
    /// `TypeMismatch` if the stored value's type differs from the requested
    /// one — never coerces.
    pub fn get_value(env: Env, namespace: Symbol, key: Symbol, ty: ValueType) -> Option<ParamValue> {
        ttl::extend_instance(&env);
        let storage_key = DataKey::Typed(TypedKey::new(namespace, key, ty));
        let stored: Option<ParamValue> = ttl::get_persistent(&env, &storage_key);
        match stored {
            Some(v) if v.type_tag() == ty => Some(v),
            Some(_) => panic_with_error!(&env, Error::TypeMismatch),
            None => None,
        }
    }

    /// Reads a typed value, falling back to `default` when unset. The
    /// default must match `ty`; a mismatch fails closed.
    pub fn get_value_or(
        env: Env,
        namespace: Symbol,
        key: Symbol,
        ty: ValueType,
        default: ParamValue,
    ) -> ParamValue {
        ttl::extend_instance(&env);
        if default.type_tag() != ty {
            panic_with_error!(&env, Error::TypeMismatch);
        }
        let storage_key = DataKey::Typed(TypedKey::new(namespace, key, ty));
        let stored: Option<ParamValue> = ttl::get_persistent(&env, &storage_key);
        match stored {
            Some(v) if v.type_tag() == ty => v,
            Some(_) => panic_with_error!(&env, Error::TypeMismatch),
            None => default,
        }
    }

    /// Removes a typed value. Admin or multisig only. Removing an unset
    /// key is a no-op that still emits an event with `old = None`.
    pub fn remove_value(env: Env, namespace: Symbol, key: Symbol, ty: ValueType) {
        ttl::extend_instance(&env);
        zenith_common::require_admin_or_multisig(&env);
        migrations::require_current(&env);
        let storage_key = DataKey::Typed(TypedKey::new(namespace.clone(), key.clone(), ty));
        let old = ttl::get_persistent::<ParamValue>(&env, &storage_key);
        if old.is_some() {
            env.storage().persistent().remove(&storage_key);
            Self::bump_version(&env);
        }
        events::value_removed(&env, namespace, key, ty, old);
    }

    /// Lists up to `limit` typed values in `namespace`, starting at
    /// `cursor`. Permissionless. Returns `(entries, next_cursor)`; a
    /// `next_cursor` of `None` means the listing is exhausted. `limit == 0`
    /// uses `DEFAULT_LIST_LIMIT`; values above `MAX_LIST_LIMIT` are clamped.
    pub fn list_namespace(
        env: Env,
        namespace: Symbol,
        cursor: u32,
        limit: u32,
    ) -> (Vec<(Symbol, ParamValue)>, Option<u32>) {
        ttl::extend_instance(&env);
        let cap = if limit == 0 {
            DEFAULT_LIST_LIMIT
        } else if limit > MAX_LIST_LIMIT {
            MAX_LIST_LIMIT
        } else {
            limit
        };
        let index_key = DataKey::NamespaceIndex(namespace.clone());
        let keys: Vec<TypedKey> = ttl::get_persistent(&env, &index_key).unwrap_or(Vec::new(&env));
        let total = keys.len();
        let mut out: Vec<(Symbol, ParamValue)> = Vec::new(&env);
        let mut i = cursor;
        while i < total && out.len() < cap {
            let typed = keys.get(i).unwrap();
            let storage_key = DataKey::Typed(typed.clone());
            if let Some(v) = ttl::get_persistent::<ParamValue>(&env, &storage_key) {
                out.push_back((typed.key, v));
            }
            i += 1;
        }
        let next = if i < total { Some(i) } else { None };
        (out, next)
    }

    pub fn get_param(env: Env, key: Symbol) -> Option<Param> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::Param(key))
    }

    pub fn get_value(env: Env, key: Symbol) -> i128 {
        ttl::extend_instance(&env);
        Self::load(&env, &key).value
    }

    pub fn get_pending_bounds(env: Env, key: Symbol) -> Option<BoundsProposal> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::PendingBounds(key))
    }

    pub fn get_version(env: Env) -> u64 {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Version).unwrap_or(0)
    }

    pub fn get_timelock(env: Env) -> Address {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Timelock).unwrap()
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Timelock only. Returns
    /// the version reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, from: u32, to: u32) -> u32 {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
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
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> u32 {
        ttl::extend_instance(&env);
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    pub fn schema_version(env: Env) -> u32 {
        ttl::extend_instance(&env);
        zenith_common::migrations::schema_version(&env)
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        ttl::extend_instance(&env);
        zenith_common::migrations::migration_state(&env)
    }
}

impl Params {
    fn require_timelock(env: &Env) {
        let timelock: Address = env.storage().instance().get(&DataKey::Timelock).unwrap();
        timelock.require_auth();
    }

    fn load(env: &Env, key: &Symbol) -> Param {
        ttl::get_persistent(env, &DataKey::Param(key.clone()))
            .unwrap_or_else(|| panic_with_error!(env, Error::ParamNotFound))
    }

    fn bump_version(env: &Env) {
        let version: u64 = env.storage().instance().get(&DataKey::Version).unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::Version, &(version + 1));
    }

    /// Adds `typed` to the namespace index if not already present.
    fn index_typed(env: &Env, typed: &TypedKey) {
        let index_key = DataKey::NamespaceIndex(typed.namespace.clone());
        let mut keys: Vec<TypedKey> = ttl::get_persistent(env, &index_key).unwrap_or(Vec::new(env));
        if !keys.contains(typed) {
            keys.push_back(typed.clone());
            ttl::set_persistent(env, &index_key, &keys);
        }
    }
}
