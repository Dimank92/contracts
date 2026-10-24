//! Events, schema v2 (docs/events.md): topics `(name, EVENTS_VERSION,
//! key...)`, data a named struct. Stake events carry the resulting
//! balances and reward events the resulting accumulator, so an indexer
//! can compute every staker's accrual without reads.

use soroban_sdk::{contracttype, Address, Env, Symbol};
use zenith_common::events::{topics, EVENTS_VERSION};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializedEvent {
    pub stake_token: Address,
    pub splitter: Address,
    pub cooldown: u64,
    pub schema_version: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StakedEvent {
    pub amount: i128,
    /// The user's earning stake after this call.
    pub stake: i128,
    pub total_staked: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnstakeRequestedEvent {
    pub amount: i128,
    /// The user's earning stake after this call.
    pub stake: i128,
    /// The user's whole pending unstake (earlier requests included).
    pub pending: i128,
    pub unlock_at: u64,
    pub total_staked: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawnEvent {
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RewardNotifiedEvent {
    pub amount: i128,
    /// `RewardPerToken(token)` after this notification.
    pub reward_per_token: i128,
    /// Rewards still carried over (nothing was staked to distribute to).
    pub undistributed: i128,
    pub total_staked: i128,
}

/// Rewards carried over while nothing was staked were distributed to the
/// first stake.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CarryoverDistributedEvent {
    pub amount: i128,
    pub reward_per_token: i128,
    pub total_staked: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RewardClaimedEvent {
    pub amount: i128,
}

fn keyed(env: &Env, name: &str, key: Address) -> (Symbol, u32, Address) {
    (Symbol::new(env, name), EVENTS_VERSION, key)
}

pub fn initialized(env: &Env, data: InitializedEvent) {
    env.events().publish(topics(env, "initialized"), data);
}

pub fn staked(env: &Env, user: Address, data: StakedEvent) {
    env.events().publish(keyed(env, "staked", user), data);
}

pub fn unstake_requested(env: &Env, user: Address, data: UnstakeRequestedEvent) {
    env.events()
        .publish(keyed(env, "unstake_requested", user), data);
}

pub fn withdrawn(env: &Env, user: Address, data: WithdrawnEvent) {
    env.events().publish(keyed(env, "withdrawn", user), data);
}

pub fn reward_notified(env: &Env, token: Address, data: RewardNotifiedEvent) {
    env.events()
        .publish(keyed(env, "reward_notified", token), data);
}

pub fn carryover_distributed(env: &Env, token: Address, data: CarryoverDistributedEvent) {
    env.events()
        .publish(keyed(env, "carryover_distributed", token), data);
}

pub fn reward_claimed(env: &Env, user: Address, token: Address, data: RewardClaimedEvent) {
    env.events().publish(
        (
            Symbol::new(env, "reward_claimed"),
            EVENTS_VERSION,
            user,
            token,
        ),
        data,
    );
}
