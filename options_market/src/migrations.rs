//! Storage-schema migration steps for this contract. The framework
//! (version checks, ordering, pagination, the in-progress guard) lives in
//! `zenith_common::migrations`; this file only registers steps. See
//! docs/migrations.md for how to author one.
//!
//! NOTE: this crate's lib.rs, error.rs, storage.rs and types.rs are
//! empty on main (emptied by #149). Once they're restored:
//! - add `mod migrations;` to lib.rs, the migration error variants (codes
//!   100–104, same names as every other crate) to `Error`, and
//!   `zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION)`
//!   to `initialize`;
//! - rewire the existing `upgrade` / `upgrade_via_multisig` onto
//!   `zenith_common::upgrade::upgrade` (with `UpgradeAuth::Admin` /
//!   `UpgradeAuth::PinnedMultisig`), which drops the caller-supplied
//!   `multisig_contract` argument in favour of the pin set here — an ABI
//!   change to list in tools/spec-diff/allow.txt;
//! - call `migrations::require_current` from state-changing entrypoints.
//!
//! Until `initialize` writes it, a missing `SchemaVersion` reads as
//! `BASELINE_SCHEMA_VERSION` (1), which is also correct for
//! already-deployed instances.

use soroban_sdk::{contractimpl, panic_with_error, Address, Env};
use zenith_common::migrations::{self, MigrationErrors, Step};
use zenith_common::upgrade;
use zenith_common::{require_admin_or_multisig, ActionClass, Auth};

use crate::error::Error;
use crate::types::DataKey;
use crate::{ttl, OptionsMarket, OptionsMarketClient};

/// The layout this wasm reads and writes. Raise it by one in the same PR
/// that adds the matching arm to `step`.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

pub const ERRORS: MigrationErrors<Error> = MigrationErrors {
    version_mismatch: Error::SchemaVersionMismatch,
    invalid_target: Error::InvalidMigrationTarget,
    in_progress: Error::MigrationInProgress,
    not_in_progress: Error::NoMigrationInProgress,
    invalid_batch: Error::InvalidMigrationBatch,
};

/// Step registry, keyed by the version each step migrates **to**. Layout
/// 1 is the baseline, so there are no steps yet. Add one arm per version:
///
/// ```ignore
/// 2 => v2(env, cursor, limit),
/// ```
///
/// A step over unbounded data processes items `[cursor, cursor + limit)`
/// and returns `Step::More(cursor + processed)` until it's done.
pub fn step(env: &Env, to: u32, cursor: u32, limit: u32) -> Step {
    let _ = (cursor, limit);
    #[allow(clippy::match_single_binding)]
    match to {
        _ => panic_with_error!(env, Error::InvalidMigrationTarget),
    }
}

/// The `MigrationInProgress` guard for state-changing entrypoints.
pub fn require_current(env: &Env) {
    migrations::require_schema_current(env, CURRENT_SCHEMA_VERSION, Error::MigrationInProgress);
}

fn require_admin(env: &Env) {
    zenith_common::get_admin(env, &DataKey::Admin).require_auth();
}

/// Migration entrypoints. A separate `#[contractimpl]` block so the
/// whole upgrade surface lives in one file.
#[contractimpl]
impl OptionsMarket {
    /// Pins the multisig `migrate_via_multisig` (and, once rewired,
    /// `upgrade_via_multisig`) trusts. Admin only; re-pinning replaces it.
    pub fn set_upgrade_multisig(env: Env, multisig: Address) {
        ttl::extend_instance(&env);
        require_admin(&env);
        upgrade::pin_multisig(&env, &multisig);
    }

    pub fn get_upgrade_multisig(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        upgrade::pinned_multisig(&env)
    }

    /// Runs the registered steps `from+1 ..= to`. Admin only. Returns the
    /// version reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, from: u32, to: u32) -> u32 {
        ttl::extend_instance(&env);
        require_admin(&env);
        migrations::migrate(&env, from, to, CURRENT_SCHEMA_VERSION, step, &ERRORS)
    }

    /// Permissionless alternative to `migrate`, authorized by the pinned
    /// multisig. `Emergency` class (no extra delay): the steps it runs
    /// were already timelocked as `Critical` with the upgrade that
    /// installed them, and the contract is blocked until they run.
    pub fn migrate_via_multisig(env: Env, action_id: u64, from: u32, to: u32) -> u32 {
        ttl::extend_instance(&env);
        let multisig = upgrade::pinned_multisig(&env)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Unauthorized));
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig, action_id),
            ActionClass::Emergency,
            Error::Unauthorized,
        );
        migrations::migrate(&env, from, to, CURRENT_SCHEMA_VERSION, step, &ERRORS)
    }

    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by `migrate` and the steps are code in
    /// this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> u32 {
        ttl::extend_instance(&env);
        migrations::migrate_batch(&env, cursor, limit, step, &ERRORS)
    }

    pub fn schema_version(env: Env) -> u32 {
        ttl::extend_instance(&env);
        migrations::schema_version(&env)
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        ttl::extend_instance(&env);
        migrations::migration_state(&env)
    }
}
