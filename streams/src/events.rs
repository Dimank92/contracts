//! Events, schema v2 (docs/events.md): topics `(name, EVENTS_VERSION,
//! stream_id)`, data a named struct.

use soroban_sdk::{contracttype, Address, Env, Symbol};
use zenith_common::events::{topics, EVENTS_VERSION};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializedEvent {
    pub governance: Address,
    pub schema_version: u32,
}

/// The whole new stream; it starts with nothing withdrawn.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamCreatedEvent {
    pub funder: Address,
    pub recipient: Address,
    pub token: Address,
    pub total: i128,
    pub start: u64,
    pub cliff: u64,
    pub end: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawnEvent {
    pub recipient: Address,
    pub amount: i128,
    /// Cumulative amount withdrawn from the stream after this call.
    pub withdrawn: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamCanceledEvent {
    pub caller: Address,
    pub to_recipient: i128,
    pub to_funder: i128,
    /// The frozen total (and withdrawn amount) after cancellation.
    pub streamed: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamTransferredEvent {
    pub from: Address,
    pub to: Address,
}

fn keyed(env: &Env, name: &str, stream_id: u64) -> (Symbol, u32, u64) {
    (Symbol::new(env, name), EVENTS_VERSION, stream_id)
}

pub fn initialized(env: &Env, data: InitializedEvent) {
    env.events().publish(topics(env, "initialized"), data);
}

pub fn created(env: &Env, stream_id: u64, data: StreamCreatedEvent) {
    env.events()
        .publish(keyed(env, "stream_created", stream_id), data);
}

pub fn withdrawn(env: &Env, stream_id: u64, data: WithdrawnEvent) {
    env.events()
        .publish(keyed(env, "withdrawn", stream_id), data);
}

pub fn canceled(env: &Env, stream_id: u64, data: StreamCanceledEvent) {
    env.events()
        .publish(keyed(env, "stream_canceled", stream_id), data);
}

pub fn transferred(env: &Env, stream_id: u64, data: StreamTransferredEvent) {
    env.events()
        .publish(keyed(env, "stream_transferred", stream_id), data);
}

pub fn topped_up(env: &Env, stream_id: u64, from: Address, amount: i128, new_end: u64) {
    env.events().publish(
        (Symbol::new(env, "topped_up"), stream_id),
        (from, amount, new_end),
    );
}

pub fn duration_extended(env: &Env, stream_id: u64, new_end: u64) {
    env.events()
        .publish((Symbol::new(env, "duration_extended"), stream_id), new_end);
}

pub fn rate_changed(env: &Env, stream_id: u64, from: Address, added: i128, new_rate: i128) {
    env.events().publish(
        (Symbol::new(env, "rate_changed"), stream_id),
        (from, added, new_rate),
    );
}
