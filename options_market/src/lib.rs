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

#[contract]
pub struct OptionsMarket;

#[contractimpl]
impl OptionsMarket {
    /// Buyer entrypoint: pays the admin-configured per-series premium and records
    /// the buyer's long position. The premium is escrowed under the per-series
    /// tag via the vault integration so it cannot be double-counted or claimed
    /// twice. Rejects halted, expired, or unknown series.
    pub fn buy_option(
        env: Env,
        buyer: Address,
        series_id: BytesN<32>,
        amount: i128,
    ) -> Result<u64, Error> {
        buyer.require_auth();
        require_not_paused(&env)?;

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        // Rejects unknown, halted, and expired series (expiry boundary included).
        let series = require_active_series(&env, &series_id)?;

        // Premium comes straight from the admin-configured per-series value;
        // the buyer pays exactly `premium * amount` with no rounding drift.
        let premium = series.premium;
        if premium <= 0 {
            return Err(Error::InvalidPremium);
        }
        let total_premium = premium
            .checked_mul(amount)
            .ok_or(Error::Overflow)?;

        // Escrow the premium under the per-series tag via the vault integration.
        let vault = vault_client::Client::new(&env, &series.vault);
        let token_client = token::Client::new(&env, &series.collateral_token);
        token_client.transfer(&buyer, &series.vault, &total_premium);
        vault.deposit(&series_id, &buyer, &total_premium);

        // Record the buyer's position with TTL extension.
        let position_id = next_position_id(&env);
        let position = OptionPosition {
            id: position_id,
            series_id: series_id.clone(),
            owner: buyer.clone(),
            side: PositionSide::Long,
            amount,
            premium_paid: total_premium,
            created_at: env.ledger().timestamp(),
            settled: false,
        };
        add_user_position(&env, &buyer, &position);

        events::emit_position_opened(&env, &buyer, &series_id, position_id, total_premium);

        Ok(position_id)
    }
}
