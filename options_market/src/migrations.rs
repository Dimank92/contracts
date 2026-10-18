//! Storage-schema migration steps for this contract. The framework
//! (version checks, ordering, pagination, the in-progress guard) lives in
//! `zenith_common::migrations`; this file only registers steps. See
//! docs/migrations.md for how to author one.

use soroban_sdk::{contractimpl, panic_with_error, Address, BytesN, Env};
use zenith_common::migrations::{self, MigrationErrors, Step};
use zenith_common::upgrade::{self, UpgradeAuth};
use zenith_common::{self as common, require_admin_or_multisig, ActionClass, Auth};

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
    require_admin_or_multisig(
        env,
        &DataKey::Admin,
        &Auth::Admin,
        ActionClass::Critical,
        Error::Unauthorized,
    );
}

/// Upgrade, migration and trusted-multisig entrypoints. A separate
/// `#[contractimpl]` block so the whole upgrade surface lives in one file.
#[contractimpl]
impl OptionsMarket {
    /// Swaps this contract's wasm, keeping its address and storage.
    /// Admin only. Follow with `migrate` if the new wasm raises
    /// `CURRENT_SCHEMA_VERSION` — until then state-changing entrypoints
    /// fail with `MigrationInProgress`.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        upgrade::upgrade(
            &env,
            &DataKey::Admin,
            UpgradeAuth::Admin,
            new_wasm_hash,
            Error::Unauthorized,
        );
    }

    /// Permissionless alternative to `upgrade`, authorized by the trusted
    /// multisig (`set_multisig`) approving `action_id`, which must equal
    /// `upgrade_action_id(new_wasm_hash)` — so the approval covers
    /// exactly this wasm on exactly this contract.
    pub fn upgrade_via_multisig(env: Env, action_id: BytesN<32>, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        upgrade::upgrade(
            &env,
            &DataKey::Admin,
            UpgradeAuth::PinnedMultisig(action_id),
            new_wasm_hash,
            Error::Unauthorized,
        );
    }

    /// The multisig action id that authorizes upgrading this contract to
    /// `new_wasm_hash` — what signers approve.
    pub fn upgrade_action_id(env: Env, new_wasm_hash: BytesN<32>) -> BytesN<32> {
        upgrade::upgrade_action_id(&env, &new_wasm_hash)
    }

    /// Sets the multisig every `_via_multisig` entrypoint (and
    /// `upgrade_via_multisig`) trusts; a `_via_multisig` call naming any
    /// other multisig is rejected. Admin only; setting again replaces it.
    pub fn set_multisig(env: Env, multisig: Address) {
        ttl::extend_instance(&env);
        require_admin(&env);
        common::set_trusted_multisig(&env, &multisig);
    }

    pub fn get_multisig(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        common::trusted_multisig(&env)
    }

    /// Runs the registered steps `from+1 ..= to`. Admin only. Returns the
    /// version reached (less than `to` if a step paginated).
    pub fn migrate(env: Env, from: u32, to: u32) -> u32 {
        ttl::extend_instance(&env);
        require_admin(&env);
        migrations::migrate(&env, from, to, CURRENT_SCHEMA_VERSION, step, &ERRORS)
    }

    /// Permissionless alternative to `migrate`, authorized by the trusted
    /// multisig. `Emergency` class: the steps it runs were already
    /// approved as `Critical` with the upgrade that installed them, and
    /// the contract is blocked until they run.
    pub fn migrate_via_multisig(env: Env, action_id: BytesN<32>, from: u32, to: u32) -> u32 {
        ttl::extend_instance(&env);
        let multisig = common::trusted_multisig(&env)
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
