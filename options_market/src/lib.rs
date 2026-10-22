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
};
use types::{DataKey, OptionPosition, OptionSeries, OptionType, PositionSide, SeriesState};

/// Maximum number of series ids a single `list_series` page may return.
/// Caps the page size in code so a caller cannot exhaust the resource budget.
const MAX_SERIES_PAGE: u32 = 50;

#[contract]
pub struct OptionsMarket;

#[contractimpl]
impl OptionsMarket {
    /// Open a collateralized writer position in an existing series.
    ///
    /// The writer locks the collateral required for the series' option type
    /// (call = underlying amount; put = strike x amount in the quote token)
    /// and receives an on-chain position record. Collateral is escrowed in the
    /// `vault` contract under the namespaced tag `(options_market, "series", series_id)`.
    pub fn write(
        env: Env,
        writer: Address,
        series_id: u64,
        amount: i128,
    ) -> Result<u64, Error> {
        writer.require_auth();
        require_not_paused(&env)?;

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let series = require_active_series(&env, series_id)?;

        // Reject writes against expired series.
        if env.ledger().timestamp() >= series.expiry {
            return Err(Error::SeriesExpired);
        }

        // Enforce per-series write cap on-chain.
        let new_written = series.total_written.checked_add(amount).ok_or(Error::Overflow)?;
        if new_written > series.max_written {
            return Err(Error::WriteCapExceeded);
        }

        // Compute collateral per option type in fixed point (no panic paths).
        let collateral = math::calc_collateral(&series, amount)?;

        // Escrow collateral in the vault under the series tag.
        let tag = Self::series_tag(&env, series_id);
        let vault = vault_client::Client::new(&env, &series.vault);
        vault.deposit(&writer, &series.collateral_token, &collateral, &tag);

        // Record the writer position keyed by (series_id, writer).
        let key = DataKey::WriterPosition(series_id, writer.clone());
        let mut position = env
            .storage()
            .persistent()
            .get::<DataKey, OptionPosition>(&key)
            .unwrap_or(OptionPosition {
                series_id,
                owner: writer.clone(),
                side: PositionSide::Writer,
                amount: 0,
                collateral: 0,
                sold: false,
            });
        position.amount = position.amount.checked_add(amount).ok_or(Error::Overflow)?;
        position.collateral = position
            .collateral
            .checked_add(collateral)
            .ok_or(Error::Overflow)?;
        env.storage().persistent().set(&key, &position);
        env.storage()
            .persistent()
            .extend_ttl(&key, SETTLEMENT_WINDOW, SETTLEMENT_WINDOW);

        // Update series accounting.
        let mut updated = series.clone();
        updated.total_written = new_written;
        env.storage().persistent().set(&DataKey::Series(series_id), &updated);
        env.storage().persistent().extend_ttl(
            &DataKey::Series(series_id),
            SETTLEMENT_WINDOW,
            SETTLEMENT_WINDOW,
        );

        let position_id = next_position_id(&env);
        add_user_position(&env, &writer, position_id);

        events::writer_position_opened(&env, series_id, &writer, amount, collateral);

        Ok(position_id)
    }

    /// Alias for `write`.
    pub fn mint(
        env: Env,
        writer: Address,
        series_id: u64,
        amount: i128,
    ) -> Result<u64, Error> {
        Self::write(env, writer, series_id, amount)
    }

    /// Release a writer position that has not yet been sold, returning escrowed
    /// collateral from the vault under the series tag.
    pub fn refund(
        env: Env,
        writer: Address,
        series_id: u64,
        amount: i128,
    ) -> Result<(), Error> {
        writer.require_auth();
        require_not_paused(&env)?;

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let series = require_active_series(&env, series_id)?;

        let key = DataKey::WriterPosition(series_id, writer.clone());
        let mut position = env
            .storage()
            .persistent()
            .get::<DataKey, OptionPosition>(&key)
            .ok_or(Error::PositionNotFound)?;

        if position.sold {
            return Err(Error::PositionAlreadySold);
        }
        if amount > position.amount {
            return Err(Error::InsufficientPosition);
        }

        // Release proportional collateral for the refunded amount.
        let release = math::calc_collateral(&series, amount)?;
        if release > position.collateral {
            return Err(Error::InsufficientPosition);
        }

        let tag = Self::series_tag(&env, series_id);
        let vault = vault_client::Client::new(&env, &series.vault);
        vault.withdraw(&writer, &series.collateral_token, &release, &tag);

        position.amount = position.amount.checked_sub(amount).ok_or(Error::Overflow)?;
        position.collateral = position
            .collateral
            .checked_sub(release)
            .ok_or(Error::Overflow)?;

        if position.amount == 0 {
            env.storage().persistent().remove(&key);
        } else {
            env.storage().persistent().set(&key, &position);
            env.storage()
                .persistent()
                .extend_ttl(&key, SETTLEMENT_WINDOW, SETTLEMENT_WINDOW);
        }

        let mut updated = series.clone();
        updated.total_written = updated
            .total_written
            .checked_sub(amount)
            .ok_or(Error::Overflow)?;
        env.storage().persistent().set(&DataKey::Series(series_id), &updated);
        env.storage().persistent().extend_ttl(
            &DataKey::Series(series_id),
            SETTLEMENT_WINDOW,
            SETTLEMENT_WINDOW,
        );

        events::writer_position_closed(&env, series_id, &writer, amount, release);

        Ok(())
    }

    /// Alias for `refund`.
    pub fn close(
        env: Env,
        writer: Address,
        series_id: u64,
        amount: i128,
    ) -> Result<(), Error> {
        Self::refund(env, writer, series_id, amount)
    }

    /// Read a writer position record.
    pub fn get_writer_position(
        env: Env,
        series_id: u64,
        writer: Address,
    ) -> Option<OptionPosition> {
        env.storage()
            .persistent()
            .get(&DataKey::WriterPosition(series_id, writer))
    }

    /// Read a single series configuration and state.
    ///
    /// Returns `None` for ids that were never allocated. Pure read: no auth,
    /// no TTL extension, no cross-contract calls.
    pub fn get_series(env: Env, series_id: u64) -> Option<OptionSeries> {
        env.storage().persistent().get(&DataKey::Series(series_id))
    }

    /// Enumerate allocated series ids in `[start, start + limit)`.
    ///
    /// `limit` is capped at `MAX_SERIES_PAGE` in code so a large series count
    /// cannot exhaust the resource budget. Ids that were never allocated are
    /// skipped; the returned vector is bounded by the effective page size.
    pub fn list_series(env: Env, start: u64, limit: u32) -> Vec<u64> {
        let mut ids = Vec::new(&env);
        let effective = if limit > MAX_SERIES_PAGE {
            MAX_SERIES_PAGE
        } else {
            limit
        };
        let mut id = start;
        let mut scanned: u32 = 0;
        while scanned < effective {
            if env
                .storage()
                .persistent()
                .has(&DataKey::Series(id))
            {
                ids.push_back(id);
            }
            id = match id.checked_add(1) {
                Some(next) => next,
                None => break,
            };
            scanned += 1;
        }
        ids
    }

    /// Read the settlement price for a series.
    ///
    /// Returns `None` if the series has not been settled yet.
    pub fn get_settlement_price(env: Env, series_id: u64) -> Option<i128> {
        env.storage()
            .persistent()
            .get(&DataKey::SettlementPrice(series_id))
    }

    /// Read whether a series has been settled.
    pub fn is_settled(env: Env, series_id: u64) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::SettlementPrice(series_id))
    }

    /// Read whether a writer position has been claimed after settlement.
    pub fn is_writer_claimed(env: Env, series_id: u64, writer: Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::WriterClaimed(series_id, writer))
    }

    /// Read whether a buyer position has been claimed after settlement.
    pub fn is_buyer_claimed(env: Env, series_id: u64, buyer: Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::BuyerClaimed(series_id, buyer))
    }

    /// Read a buyer position record.
    pub fn get_buyer_position(
        env: Env,
        series_id: u64,
        buyer: Address,
    ) -> Option<OptionPosition> {
        env.storage()
            .persistent()
            .get(&DataKey::BuyerPosition(series_id, buyer))
    }

    fn series_tag(env: &Env, series_id: u64) -> Vec<Symbol> {
        let mut tag = Vec::new(env);
        tag.push_back(Symbol::new(env, "options_market"));
        tag.push_back(Symbol::new(env, "series"));
        tag.push_back(Symbol::new(env, "series_id"));
        tag
    }
}
