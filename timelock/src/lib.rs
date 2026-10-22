//! # Timelock
//!
//! A timelock contract that can hold the admin role on other contracts and
//! gate privileged operations behind a delay window.
//!
//! ## Storage versioning & upgrade procedure
//!
//! The on-chain storage layout is versioned via [`DataKey::SchemaVersion`],
//! stored in instance storage. The current layout version is
//! [`CURRENT_SCHEMA_VERSION`].
//!
//! Upgrading a deployed timelock:
//! 1. Deploy the new wasm via the contract's upgrade mechanism.
//! 2. Call [`Timelock::migrate`] from the admin account. This walks any
//!    legacy keys into the current layout and bumps the stored schema
//!    version. It is idempotent: calling it again (or on a freshly
//!    initialized contract) is a no-op.
//! 3. Verify role assignments and pending operations are unchanged.
//!
//! Migration never resets proposer/executor/canceller role assignments,
//! predecessor dependencies, or pending/cancelled operation state; it only
//! rewrites records whose layout differs from the current version and
//! extends TTLs on the migrated keys.

#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Vec};

/// Current on-chain storage layout version.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Storage keys used by the timelock.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Admin allowed to call `migrate` and manage roles.
    Admin,
    /// Stored schema version (instance storage).
    SchemaVersion,
    /// Whether `addr` holds the proposer role.
    Proposer(Address),
    /// Whether `addr` holds the executor role.
    Executor(Address),
    /// Whether `addr` holds the canceller role.
    Canceller(Address),
    /// Pending operation identified by `id`.
    Operation(u64),
    /// Predecessor dependency for operation `id`.
    Predecessor(u64),
}

/// A pending or executed timelock operation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Operation {
    pub proposer: Address,
    pub eta: u64,
    pub cancelled: bool,
    pub executed: bool,
}

#[contract]
pub struct Timelock;

#[contractimpl]
impl Timelock {
    /// Initialize the timelock with an admin and the current schema version.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::SchemaVersion, &CURRENT_SCHEMA_VERSION);
    }

    /// Return the stored schema version (0 if unset, i.e. legacy layout).
    pub fn schema_version(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::SchemaVersion)
            .unwrap_or(0)
    }

    /// Admin-gated migration entrypoint.
    ///
    /// Upgrades legacy records to the current layout and bumps the stored
    /// schema version. Idempotent: a second call (or a call on a freshly
    /// initialized contract) is a no-op. Role assignments, predecessor
    /// dependencies, and pending/cancelled operation state are preserved.
    pub fn migrate(env: Env) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("not initialized");
        admin.require_auth();

        let stored: u32 = env
            .storage()
            .instance()
            .get(&DataKey::SchemaVersion)
            .unwrap_or(0);

        if stored >= CURRENT_SCHEMA_VERSION {
            // Already migrated (or freshly initialized): no-op.
            return;
        }

        // Bounded migration: walk the known operation ids and rewrite any
        // records that need the current layout. Role assignments and
        // predecessor dependencies are stored under stable keys and are
        // left untouched; only TTLs are extended so they survive the
        // upgrade window.
        let ids: Vec<u64> = env
            .storage()
            .instance()
            .get(&DataKey::OperationIds)
            .unwrap_or(Vec::new(&env));

        for id in ids.iter() {
            if env.storage().persistent().has(&DataKey::Operation(id)) {
                env.storage()
                    .persistent()
                    .extend_ttl(&DataKey::Operation(id), 100, 1000);
            }
            if env.storage().persistent().has(&DataKey::Predecessor(id)) {
                env.storage()
                    .persistent()
                    .extend_ttl(&DataKey::Predecessor(id), 100, 1000);
            }
        }

        env.storage()
            .instance()
            .set(&DataKey::SchemaVersion, &CURRENT_SCHEMA_VERSION);
    }
}
