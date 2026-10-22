#![no_std]

//! Zenith Protocol — Decentralized Options Market on Stellar Soroban
//!
//! Supports European-style put and call options on XLM, BTC, ETH, and SOL.
//! Premium is set by the admin (computed off-chain via Black-Scholes).
//! Writers lock collateral; buyers pay premium. Settlement at expiry via oracle.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, token, Address, BytesN, Env, Symbol, Vec,
};

mod error;
mod events;
mod math;
mod multisig_client;
mod price_oracle_client;
mod storage;
mod types;
mod vault_client;

use error::Error;
use math::{
    calc_fee, calc_payout, DEFAULT_FEE_RATE_BPS, MAX_BATCH_SIZE, MAX_FEE_RATE_BPS,
    MAX_SERIES_PER_UNDERLYING, MIN_COLLATERAL_RATIO, PRICE_PRECISION, RATE_PRECISION,
    SETTLEMENT_WINDOW,
};
use storage::{
    add_user_position, fee_rate_bps, next_position_id, require_active_series, require_not_paused,
}

#[cfg(test)]
mod test;

//! Zenith Protocol — Decentralized Options Market on Stellar Soroban
//!
//! Supports European-style put and call options on XLM, BTC, ETH, and SOL.
//! Premium is set by the admin (computed off-chain via Black-Scholes).
//! Writers lock collateral; buyers pay premium. Settlement at expiry via oracle.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, token, Address, BytesN, Env, Symbol, Vec,
};

mod error;
mod events;
mod math;
mod multisig_client;
mod price_oracle_client;
mod storage;
mod types;
mod vault_client;

use error::Error;
use math::{
    calc_fee, calc_payout, DEFAULT_FEE_RATE_BPS, MAX_BATCH_SIZE, MAX_FEE_RATE_BPS,
    MAX_SERIES_PER_UNDERLYING, MIN_COLLATERAL_RATIO, PRICE_PRECISION, RATE_PRECISION,
    SETTLEMENT_WINDOW,
};
use storage::{
    add_user_position, fee_rate_bps, next_position_id, require_active_series, require_not_paused,
};
use types::{DataKey, OptionPosition, OptionSeries, OptionType, PositionSide, SeriesState};

#[contractimpl]
impl OptionsMarket {
    /// Top up collateral on an existing writer position.
    ///
    /// Requires writer auth, pulls `amount` of the series collateral token
    /// through the same escrow path as initial collateral locking, and
    /// updates the position's recorded collateral. The series vault tag
    /// balance is incremented by the same amount.
    pub fn top_up_collateral(env: Env, position_id: u64, amount: i128) {
        require_not_paused(&env);

        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let mut position: OptionPosition = env
            .storage()
            .persistent()
            .get(&DataKey::Position(position_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::PositionNotFound));

        if position.side != PositionSide::Writer {
            panic_with_error!(&env, Error::Unauthorized);
        }

        let series: OptionSeries = env
            .storage()
            .persistent()
            .get(&DataKey::Series(position.series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        require_active_series(&env, &series);

        // Only the position's writer may top up.
        position.writer.require_auth();

        lock_collateral_delta(&env, &series, &mut position, &position.writer, amount);

        env.storage()
            .persistent()
            .set(&DataKey::Position(position_id), &position);

        events::top_up_collateral(
            &env,
            position_id,
            position.series_id,
            &position.writer,
            amount,
            position.collateral,
        );
    }

    /// Unwind a writer position before expiry.
    ///
    /// Burns the writer's position, returns collateral minus the cost of
    /// buying back the short at the current premium (computed at the current
    /// oracle price), and refuses to run after expiry or once settlement has
    /// begun. Fails closed on a stale oracle price and while the series is
    /// halted or in an emergency state.
    pub fn unwind_position(env: Env, position_id: u64) {
        require_not_paused(&env);

        let mut position: OptionPosition = env
            .storage()
            .persistent()
            .get(&DataKey::Position(position_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::PositionNotFound));

        if position.side != PositionSide::Writer {
            panic_with_error!(&env, Error::Unauthorized);
        }

        let series: OptionSeries = env
            .storage()
            .persistent()
            .get(&DataKey::Series(position.series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        // Refuse after expiry or once settlement has begun.
        let now = env.ledger().timestamp();
        if now >= series.expiry {
            panic_with_error!(&env, Error::SeriesExpired);
        }
        if series.state != SeriesState::Active {
            panic_with_error!(&env, Error::SeriesNotActive);
        }

        // Block release while the series is halted or in emergency.
        if series.halted || series.emergency {
            panic_with_error!(&env, Error::SeriesHalted);
        }

        // Only the position's writer may unwind.
        position.writer.require_auth();

        // Fail closed on a stale oracle price rather than guessing.
        let oracle = price_oracle_client::Client::new(&env, &series.oracle);
        let oracle_price = oracle.get_price(&series.underlying);
        if !oracle.is_fresh(&series.underlying) {
            panic_with_error!(&env, Error::StalePrice);
        }

        // Buy-back premium at the current oracle price.
        let buyback_premium = math::calc_buyback_premium(
            &env,
            &series,
            &position,
            oracle_price,
        );

        // Unwind that would leave collateral below zero is refused.
        if buyback_premium > position.collateral {
            panic_with_error!(&env, Error::InsufficientCollateral);
        }

        let payout = position
            .collateral
            .checked_sub(buyback_premium)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Overflow));

        // Burn the position and release the full recorded collateral, then
        // return the net payout to the writer.
        release_collateral_delta(&env, &series, &mut position, &position.writer, position.collateral);

        env.storage()
            .persistent()
            .remove(&DataKey::Position(position_id));

        let vault = vault_client::Client::new(&env, &series.vault);
        if payout > 0 {
            vault.release_collateral(&series.collateral_token, &position.writer, &payout);
        }

        events::unwind_position(
            &env,
            position_id,
            position.series_id,
            &position.writer,
            buyback_premium,
            payout,
            0,
        );
    }
}
/// Ledger entry key for the vault tag balance of a series.
///
/// The tag balance must always equal the sum of live positions' recorded
/// collateral for that series.
fn tag_balance_key(env: &Env, series_id: u64) -> DataKey {
    DataKey::SeriesTagBalance(series_id)
}

/// Read the vault tag balance for a series (0 if unset).
fn get_tag_balance(env: &Env, series_id: u64) -> i128 {
    env.storage()
        .persistent()
        .get(&tag_balance_key(env, series_id))
        .unwrap_or(0)
}

/// Write the vault tag balance for a series.
fn set_tag_balance(env: &Env, series_id: u64, amount: i128) {
    env.storage()
        .persistent()
        .set(&tag_balance_key(env, series_id), &amount);
}

/// The single write site for writer collateral accounting.
///
/// Pulls `delta` of the series collateral token from `writer` through the
/// vault escrow path (identical to initial collateral locking), then updates
/// the position's recorded collateral and the series tag balance by the same
/// delta so the invariant `tag_balance == sum(live collateral)` is preserved.
fn lock_collateral_delta(
    env: &Env,
    series: &OptionSeries,
    position: &mut OptionPosition,
    writer: &Address,
    delta: i128,
) {
    if delta <= 0 {
        panic_with_error!(env, Error::InvalidAmount);
    }

    // Pull tokens through the same escrow path used for initial locking.
    let vault = vault_client::Client::new(env, &series.vault);
    vault.lock_collateral(&series.collateral_token, writer, &delta);

    position.collateral = position
        .collateral
        .checked_add(delta)
        .unwrap_or_else(|| panic_with_error!(env, Error::Overflow));

    let new_tag = get_tag_balance(env, series.id)
        .checked_add(delta)
        .unwrap_or_else(|| panic_with_error!(env, Error::Overflow));
    set_tag_balance(env, series.id, new_tag);
}

/// Release `delta` of collateral back to `writer` and decrement the series
/// tag balance by the same delta, keeping the invariant intact.
fn release_collateral_delta(
    env: &Env,
    series: &OptionSeries,
    position: &mut OptionPosition,
    writer: &Address,
    delta: i128,
) {
    if delta < 0 {
        panic_with_error!(env, Error::InvalidAmount);
    }

    position.collateral = position
        .collateral
        .checked_sub(delta)
        .unwrap_or_else(|| panic_with_error!(env, Error::Overflow));

    let new_tag = get_tag_balance(env, series.id)
        .checked_sub(delta)
        .unwrap_or_else(|| panic_with_error!(env, Error::Overflow));
    set_tag_balance(env, series.id, new_tag);

    if delta > 0 {
        let vault = vault_client::Client::new(env, &series.vault);
        vault.release_collateral(&series.collateral_token, writer, &delta);
    }
}
