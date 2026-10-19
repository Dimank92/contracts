//! Events, schema v2 (docs/events.md): topics `(name, EVENTS_VERSION,
//! key...)`, data a named struct. Events carry the pool totals they
//! changed, so an indexer can mirror the pool without reads.

use soroban_sdk::{contracttype, Address, Env, Symbol};
use zenith_common::events::{topics, EVENTS_VERSION};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializedEvent {
    pub admin: Address,
    pub keeper: Address,
    pub asset: Address,
    pub market: Address,
    pub max_utilization_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeeperSetEvent {
    pub keeper: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaxUtilizationSetEvent {
    pub max_utilization_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeriesCapSetEvent {
    /// Zero de-lists the series.
    pub max_collateral: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiabilitiesMarkedEvent {
    pub expected_liabilities: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepositQueuedEvent {
    pub amount: i128,
    /// The user's whole queued deposit for this epoch.
    pub ticket: i128,
    pub pending_deposits: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawQueuedEvent {
    pub shares: i128,
    /// The user's whole queued withdrawal for this epoch.
    pub ticket: i128,
    /// The user's share balance after the shares left it.
    pub share_balance: i128,
    pub pending_withdraw_shares: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedEvent {
    pub shares: i128,
    pub assets: i128,
    /// The user's share balance after the claim.
    pub share_balance: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptionWrittenEvent {
    pub collateral: i128,
    pub premium: i128,
    pub idle: i128,
    pub locked: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollateralReclaimedEvent {
    pub series_id: u64,
    pub collateral: i128,
    pub returned: i128,
    pub idle: i128,
    pub locked: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpochProcessedEvent {
    /// Pool assets and share supply the epoch was priced at.
    pub total_assets: i128,
    pub total_shares: i128,
    pub deposit_assets: i128,
    pub deposit_shares: i128,
    pub withdraw_shares: i128,
    pub withdraw_assets: i128,
    /// Totals after conversion; the next epoch is `epoch + 1`.
    pub idle: i128,
    pub total_shares_after: i128,
}

fn keyed<K>(env: &Env, name: &str, key: K) -> (Symbol, u32, K) {
    (Symbol::new(env, name), EVENTS_VERSION, key)
}

pub fn initialized(env: &Env, data: InitializedEvent) {
    env.events().publish(topics(env, "initialized"), data);
}

pub fn keeper_set(env: &Env, data: KeeperSetEvent) {
    env.events().publish(topics(env, "keeper_set"), data);
}

pub fn max_utilization_set(env: &Env, data: MaxUtilizationSetEvent) {
    env.events()
        .publish(topics(env, "max_utilization_set"), data);
}

pub fn series_cap_set(env: &Env, series_id: u64, data: SeriesCapSetEvent) {
    env.events()
        .publish(keyed(env, "series_cap_set", series_id), data);
}

pub fn liabilities_marked(env: &Env, data: LiabilitiesMarkedEvent) {
    env.events()
        .publish(topics(env, "liabilities_marked"), data);
}

pub fn deposit_queued(env: &Env, user: Address, epoch: u32, data: DepositQueuedEvent) {
    env.events().publish(
        (
            Symbol::new(env, "deposit_queued"),
            EVENTS_VERSION,
            user,
            epoch,
        ),
        data,
    );
}

pub fn withdraw_queued(env: &Env, user: Address, epoch: u32, data: WithdrawQueuedEvent) {
    env.events().publish(
        (
            Symbol::new(env, "withdraw_queued"),
            EVENTS_VERSION,
            user,
            epoch,
        ),
        data,
    );
}

pub fn claimed(env: &Env, user: Address, data: ClaimedEvent) {
    env.events().publish(keyed(env, "claimed", user), data);
}

pub fn option_written(env: &Env, series_id: u64, position_id: u64, data: OptionWrittenEvent) {
    env.events().publish(
        (
            Symbol::new(env, "option_written"),
            EVENTS_VERSION,
            series_id,
            position_id,
        ),
        data,
    );
}

pub fn collateral_reclaimed(env: &Env, position_id: u64, data: CollateralReclaimedEvent) {
    env.events()
        .publish(keyed(env, "collateral_reclaimed", position_id), data);
}

pub fn epoch_processed(env: &Env, epoch: u32, data: EpochProcessedEvent) {
    env.events()
        .publish(keyed(env, "epoch_processed", epoch), data);
}
