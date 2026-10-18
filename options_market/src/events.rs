use soroban_sdk::{Address, Env, Symbol};

use crate::types::{OptionPosition, OptionSeries};

pub fn series_created(env: &Env, series_id: u64, strike_price: i128, expiry: u64, premium: i128) {
    env.events().publish(
        (Symbol::new(env, "series_created"),),
        (series_id, strike_price, expiry, premium),
    );
}

pub fn premium_updated(env: &Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
    env.events().publish(
        (Symbol::new(env, "premium_updated"), series_id),
        (new_premium, new_implied_vol),
    );
}

pub fn option_bought(
    env: &Env,
    buyer: Address,
    pos_id: u64,
    series_id: u64,
    contracts: i128,
    total_premium: i128,
) {
    env.events().publish(
        (Symbol::new(env, "option_bought"), buyer),
        (pos_id, series_id, contracts, total_premium),
    );
}

pub fn option_written(
    env: &Env,
    writer: Address,
    pos_id: u64,
    series_id: u64,
    contracts: i128,
    writer_premium: i128,
    required_collateral: i128,
) {
    env.events().publish(
        (Symbol::new(env, "option_written"), writer),
        (
            pos_id,
            series_id,
            contracts,
            writer_premium,
            required_collateral,
        ),
    );
}

pub fn option_exercised(
    env: &Env,
    owner: Address,
    position_id: u64,
    settlement_price: i128,
    payout: i128,
) {
    env.events().publish(
        (Symbol::new(env, "option_exercised"), owner),
        (position_id, settlement_price, payout),
    );
}

pub fn settlement_price_set(env: &Env, series_id: u64, price: i128) {
    env.events()
        .publish((Symbol::new(env, "settlement_price_set"), series_id), price);
}

pub fn collateral_reclaimed(env: &Env, writer: Address, position_id: u64, reclaim: i128) {
    env.events().publish(
        (Symbol::new(env, "collateral_reclaimed"), writer),
        (position_id, reclaim),
    );
}

pub fn admin_transferred(env: &Env, old_admin: Address, new_admin: Address) {
    env.events().publish(
        (Symbol::new(env, "admin_transferred"),),
        (old_admin, new_admin),
    );
}

pub fn paused(env: &Env) {
    env.events().publish((Symbol::new(env, "paused"),), ());
}

pub fn unpaused(env: &Env) {
    env.events().publish((Symbol::new(env, "unpaused"),), ());
}

pub fn series_cancelled(env: &Env, series_id: u64) {
    env.events()
        .publish((Symbol::new(env, "series_cancelled"),), series_id);
}

pub fn refund_claimed(env: &Env, owner: Address, position_id: u64, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "refund_claimed"), owner),
        (position_id, amount),
    );
}

pub fn fee_rate_updated(env: &Env, new_bps: i128) {
    env.events()
        .publish((Symbol::new(env, "fee_rate_updated"),), new_bps);
}

pub fn series_escrowed_to_vault(env: &Env, series_id: u64, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "series_escrowed_to_vault"), series_id),
        amount,
    );
}

pub fn vault_updated(env: &Env, old_vault: Option<Address>, new_vault: Address) {
    env.events().publish(
        (Symbol::new(env, "vault_updated"),),
        (old_vault, new_vault),
    );
}
pub fn option_auto_exercised(
    env: &Env,
    owner: Address,
    position_id: u64,
    settlement_price: i128,
    payout: i128,
) {
    env.events().publish(
        (Symbol::new(env, "option_auto_exercised"), owner),
        (position_id, settlement_price, payout),
    );
}

/// Final archival record for a pruned position: carries the full struct,
/// since after this event the entry is gone from contract storage.
pub fn position_pruned(env: &Env, position: OptionPosition) {
    env.events().publish(
        (
            Symbol::new(env, "position_pruned"),
            position.owner.clone(),
            position.series_id,
        ),
        position,
    );
}

/// Final archival record for a pruned series (full struct).
pub fn series_pruned(env: &Env, series: OptionSeries) {
    env.events().publish(
        (
            Symbol::new(env, "series_pruned"),
            series.underlying.clone(),
            series.series_id,
        ),
        series,
    );
}

/// A series released its active-series slot on `underlying`;
/// `active_count` is the count after the release.
pub fn series_slot_released(env: &Env, underlying: Symbol, series_id: u64, active_count: u32) {
    env.events().publish(
        (Symbol::new(env, "series_slot_released"), underlying),
        (series_id, active_count),
    );
}

pub fn max_active_series_updated(env: &Env, cap: u32) {
    env.events()
        .publish((Symbol::new(env, "max_active_series_updated"),), cap);
}

pub fn counters_migrated(env: &Env) {
    env.events()
        .publish((Symbol::new(env, "counters_migrated"),), ());
}

/// A series was halted (`halted = true`) or unhalted (#192).
pub fn series_halted(env: &Env, series_id: u64, halted: bool) {
    env.events()
        .publish((Symbol::new(env, "series_halted"), series_id), halted);
}

pub fn price_oracle_updated(env: &Env, oracle: Address) {
    env.events()
        .publish((Symbol::new(env, "price_oracle_updated"),), oracle);
}

/// A settled series' remaining collateral moved into `vault` under the
/// series tag (#191).
pub fn settlement_escrowed_to_vault(env: &Env, series_id: u64, vault: Address, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "settlement_escrowed_to_vault"), series_id),
        (vault, amount),
    );
}
