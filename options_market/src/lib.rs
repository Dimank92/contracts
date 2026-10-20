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

#[cfg(test)]
mod test;
