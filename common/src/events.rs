//! Event schema v2 (docs/events.md, events.json), shared conventions and
//! the framework's own events.
//!
//! Every Zenith event has topics `(Symbol name, u32 EVENTS_VERSION,
//! primary key...)` and a `#[contracttype]` struct as its data, so an
//! indexer can tell schema versions apart from the topics alone and every
//! data field is named on the wire (a struct encodes as an `ScMap` keyed
//! by field name).
//!
//! The structs here are `export = false` so linking zenith-common adds no
//! entries to a consumer's contract spec; events.json documents them.

use soroban_sdk::{contracttype, Address, BytesN, Env, Symbol};

/// Second topic of every event. Bumped when any event's topics or data
/// change shape.
pub const EVENTS_VERSION: u32 = 2;

/// `(name, EVENTS_VERSION)`: the topics of an event with no primary key.
pub fn topics(env: &Env, name: &str) -> (Symbol, u32) {
    (Symbol::new(env, name), EVENTS_VERSION)
}

/// A migration step finished; the stored schema is now `to`.
#[contracttype(export = false)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaMigrated {
    pub from: u32,
    pub to: u32,
}

/// A paginated step stopped at `cursor`; `migrate_batch(cursor, ..)`
/// continues it.
#[contracttype(export = false)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationProgress {
    /// The step (version) in flight.
    pub step: u32,
    /// The version the migration will end at.
    pub target: u32,
    pub cursor: u32,
}

#[contracttype(export = false)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeMultisigPinned {
    pub multisig: Address,
}

#[contracttype(export = false)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeProposed {
    pub wasm_hash: BytesN<32>,
    pub not_before: u64,
    pub emergency: bool,
}

#[contracttype(export = false)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeCancelled {
    pub wasm_hash: BytesN<32>,
}

#[contracttype(export = false)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Upgraded {
    pub wasm_hash: BytesN<32>,
    /// Stored schema version at install, before any `migrate`.
    pub schema_version: u32,
}

#[contracttype(export = false)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeChecked {
    pub wasm_hash: BytesN<32>,
    pub schema_version: u32,
}

#[contracttype(export = false)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeCheckFailed {
    pub wasm_hash: BytesN<32>,
    /// The failed check's error code, in the contract's own registry.
    pub reason: u32,
}

pub fn schema_migrated(env: &Env, from: u32, to: u32) {
    env.events()
        .publish(topics(env, "schema_migrated"), SchemaMigrated { from, to });
}

pub fn migration_progress(env: &Env, step: u32, target: u32, cursor: u32) {
    env.events().publish(
        topics(env, "migration_progress"),
        MigrationProgress {
            step,
            target,
            cursor,
        },
    );
}

pub fn upgrade_multisig_pinned(env: &Env, multisig: Address) {
    env.events().publish(
        topics(env, "upgrade_multisig_pinned"),
        UpgradeMultisigPinned { multisig },
    );
}

pub fn upgrade_proposed(env: &Env, wasm_hash: BytesN<32>, not_before: u64, emergency: bool) {
    env.events().publish(
        (
            Symbol::new(env, "upgrade_proposed"),
            EVENTS_VERSION,
            wasm_hash.clone(),
        ),
        UpgradeProposed {
            wasm_hash,
            not_before,
            emergency,
        },
    );
}

pub fn upgrade_cancelled(env: &Env, wasm_hash: BytesN<32>) {
    env.events().publish(
        (
            Symbol::new(env, "upgrade_cancelled"),
            EVENTS_VERSION,
            wasm_hash.clone(),
        ),
        UpgradeCancelled { wasm_hash },
    );
}

pub fn upgraded(env: &Env, wasm_hash: BytesN<32>, schema_version: u32) {
    env.events().publish(
        (
            Symbol::new(env, "upgraded"),
            EVENTS_VERSION,
            wasm_hash.clone(),
        ),
        Upgraded {
            wasm_hash,
            schema_version,
        },
    );
}

pub fn upgrade_checked(env: &Env, wasm_hash: BytesN<32>, schema_version: u32) {
    env.events().publish(
        (
            Symbol::new(env, "upgrade_checked"),
            EVENTS_VERSION,
            wasm_hash.clone(),
        ),
        UpgradeChecked {
            wasm_hash,
            schema_version,
        },
    );
}

pub fn upgrade_check_failed(env: &Env, wasm_hash: BytesN<32>, reason: u32) {
    env.events().publish(
        (
            Symbol::new(env, "upgrade_check_failed"),
            EVENTS_VERSION,
            wasm_hash.clone(),
        ),
        UpgradeCheckFailed { wasm_hash, reason },
    );
}
