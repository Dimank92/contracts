//! Events, schema v2 (docs/events.md): topics `(name, EVENTS_VERSION,
//! key...)`, data a named struct. Every event carries the registry
//! `version` it produced where one applies, so an indexer can mirror the
//! registry without reads.

use soroban_sdk::{contracttype, Address, Env, Symbol};
use zenith_common::events::{topics, EVENTS_VERSION};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializedEvent {
    pub timelock: Address,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParamDefinedEvent {
    pub value: i128,
    pub min: i128,
    pub max: i128,
    pub updated_at: u64,
    /// Registry version after this change.
    pub version: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParamUpdatedEvent {
    pub old_value: i128,
    pub new_value: i128,
    pub updated_at: u64,
    pub version: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundsProposedEvent {
    pub min: i128,
    pub max: i128,
    /// When `execute_bounds` becomes callable.
    pub eta: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundsUpdatedEvent {
    pub min: i128,
    pub max: i128,
    pub updated_at: u64,
    pub version: u64,
}

/// The proposal that was dropped.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundsCancelledEvent {
    pub min: i128,
    pub max: i128,
    pub eta: u64,
}

fn keyed(env: &Env, name: &str, key: Symbol) -> (Symbol, u32, Symbol) {
    (Symbol::new(env, name), EVENTS_VERSION, key)
}

pub fn initialized(env: &Env, timelock: Address, schema_version: u32) {
    env.events().publish(
        topics(env, "initialized"),
        InitializedEvent {
            timelock,
            schema_version,
        },
    );
}

pub fn param_defined(env: &Env, key: Symbol, data: ParamDefinedEvent) {
    env.events().publish(keyed(env, "param_defined", key), data);
}

pub fn param_updated(env: &Env, key: Symbol, data: ParamUpdatedEvent) {
    env.events().publish(keyed(env, "param_updated", key), data);
}

pub fn bounds_proposed(env: &Env, key: Symbol, data: BoundsProposedEvent) {
    env.events()
        .publish(keyed(env, "bounds_proposed", key), data);
}

pub fn bounds_updated(env: &Env, key: Symbol, data: BoundsUpdatedEvent) {
    env.events()
        .publish(keyed(env, "bounds_updated", key), data);
}

pub fn bounds_cancelled(env: &Env, key: Symbol, data: BoundsCancelledEvent) {
    env.events()
        .publish(keyed(env, "bounds_cancelled", key), data);
}
