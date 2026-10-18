#![no_std]

//! Zenith Protocol — Decentralized Options Market on Stellar Soroban
//!
//! Supports European-style put and call options on XLM, BTC, ETH, and SOL.
//! Premium is set by the admin (computed off-chain via Black-Scholes).
//! Writers lock collateral; buyers pay premium. Settlement at expiry via oracle.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, token, Address, BytesN, Env, Symbol, Vec,
};

mod call_collateral;
mod error;
mod events;
mod math;
mod migrations;
mod multisig_client;
mod params_client;
mod price_oracle_client;
mod rounding_policy;
mod settlement_fallback;
mod storage;
mod ttl;
mod types;
mod vault_client;

pub use types::{DataKey, OptionPosition, OptionSeries, OptionType, PositionSide, SeriesState};

use error::Error;
use math::{
    calc_fee, calc_payout, required_collateral, DEFAULT_FEE_RATE_BPS, MAX_BATCH_SIZE,
    MAX_FEE_RATE_BPS, MAX_SERIES_PER_UNDERLYING, MIN_EXPIRY_LEAD, PRICE_PRECISION,
};
use storage::{
    active_series_count, add_user_position, decrement_active_count, fee_rate_bps, get_premium_pool,
    increment_active_count, increment_series_count, max_active_series, next_position_id,
    next_series_id, require_active_series, require_not_paused, series_counter, series_escrow,
    series_count_for_underlying, set_premium_pool, set_series_escrow,
};
use types::{OptionPosition, OptionSeries, OptionType, PositionSide, SeriesState};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_adversaries;
mod test_resources;

#[contract]
pub struct OptionsMarket;

// ─── Internal helpers ────────────────────────────────────────────────────────

fn get_admin(env: &Env) -> Address {
    zenith_common::get_admin(env, &DataKey::Admin)
}

fn get_oracle(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Oracle).unwrap()
}

fn get_collateral_token(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::CollateralToken).unwrap()
}

fn get_fee_recipient(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::FeeRecipient).unwrap()
}

fn is_valid_underlying(env: &Env, underlying: &Symbol) -> bool {
    let xlm = Symbol::new(env, "XLM");
    let btc = Symbol::new(env, "BTC");
    let eth = Symbol::new(env, "ETH");
    let sol = Symbol::new(env, "SOL");
    underlying == &xlm || underlying == &btc || underlying == &eth || underlying == &sol
}

fn create_series_inner(
    env: &Env,
    underlying: Symbol,
    option_type: OptionType,
    strike_price: i128,
    expiry: u64,
    premium: i128,
    implied_vol: i128,
) -> u64 {
    require_not_paused(env);

    let now = env.ledger().timestamp();
    if expiry <= now + MIN_EXPIRY_LEAD {
        panic_with_error!(env, Error::ExpiryTooSoon);
    }
    if strike_price <= 0 {
        panic_with_error!(env, Error::InvalidSeriesParams);
    }
    if premium <= 0 {
        panic_with_error!(env, Error::InvalidSeriesParams);
    }
    if implied_vol < 0 {
        panic_with_error!(env, Error::InvalidSeriesParams);
    }
    if !is_valid_underlying(env, &underlying) {
        panic_with_error!(env, Error::InvalidSeriesParams);
    }

    // Active-series cap per underlying
    let active = active_series_count(env, &underlying);
    let cap = max_active_series(env);
    if active >= cap {
        panic_with_error!(env, Error::TooManySeriesForUnderlying);
    }

    let series_id = next_series_id(env);
    let series = OptionSeries {
        series_id,
        underlying: underlying.clone(),
        option_type,
        strike_price,
        expiry,
        premium,
        implied_vol,
        open_interest: 0,
        state: SeriesState::Active,
        settlement_price: None,
        created_at: now,
    };
    crate::ttl::set_persistent(env, &DataKey::Series(series_id), &series);
    increment_series_count(env, &underlying);
    increment_active_count(env, &underlying);

    events::series_created(env, series_id, strike_price, expiry, premium);
    series_id
}

fn update_premium_inner(env: &Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
    let mut series = require_active_series(env, series_id);
    if new_premium <= 0 {
        panic_with_error!(env, Error::InvalidSeriesParams);
    }
    if new_implied_vol < 0 {
        panic_with_error!(env, Error::InvalidSeriesParams);
    }
    series.premium = new_premium;
    series.implied_vol = new_implied_vol;
    crate::ttl::set_persistent(env, &DataKey::Series(series_id), &series);
    events::premium_updated(env, series_id, new_premium, new_implied_vol);
}

fn cancel_series_inner(env: &Env, series_id: u64) {
    let mut series: OptionSeries =
        crate::ttl::get_persistent(env, &DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
    if series.state != SeriesState::Active {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    series.state = SeriesState::Cancelled;
    crate::ttl::set_persistent(env, &DataKey::Series(series_id), &series);
    decrement_active_count(env, &series.underlying);
    events::series_cancelled(env, series_id);
}

// ─── Main contract ───────────────────────────────────────────────────────────

#[contractimpl]
impl OptionsMarket {
    // ── Initialization ───────────────────────────────────────────────────────

    /// One-time setup. Panics with `AlreadyInitialized` if called twice.
    pub fn initialize(
        env: Env,
        admin: Address,
        oracle: Address,
        collateral_token: Address,
        fee_recipient: Address,
    ) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Oracle, &oracle);
        env.storage().instance().set(&DataKey::CollateralToken, &collateral_token);
        env.storage().instance().set(&DataKey::FeeRecipient, &fee_recipient);
        env.storage().instance().set(&DataKey::FeeRateBps, &DEFAULT_FEE_RATE_BPS);
        env.storage().instance().set(&DataKey::PremiumPool, &0i128);
        env.storage().instance().set(&DataKey::MaxActiveSeries, &MAX_SERIES_PER_UNDERLYING);
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
        crate::ttl::extend_instance(&env);
    }

    // ── Admin: series management ──────────────────────────────────────────────

    /// List a new series. Requires admin auth.
    pub fn create_series(
        env: Env,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
        premium: i128,
        implied_vol: i128,
    ) -> u64 {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        create_series_inner(
            &env,
            underlying,
            option_type,
            strike_price,
            expiry,
            premium,
            implied_vol,
        )
    }

    /// Permissionless alternative to `create_series` via multisig.
    pub fn create_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
        premium: i128,
        implied_vol: i128,
    ) -> u64 {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        create_series_inner(
            &env,
            underlying,
            option_type,
            strike_price,
            expiry,
            premium,
            implied_vol,
        )
    }

    /// Update the premium and implied vol on an active series.
    pub fn update_premium(env: Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        update_premium_inner(&env, series_id, new_premium, new_implied_vol);
    }

    pub fn update_premium_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        series_id: u64,
        new_premium: i128,
        new_implied_vol: i128,
    ) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        update_premium_inner(&env, series_id, new_premium, new_implied_vol);
    }

    /// Cancel an active series.
    pub fn cancel_series(env: Env, series_id: u64) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        cancel_series_inner(&env, series_id);
    }

    pub fn cancel_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        series_id: u64,
    ) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        cancel_series_inner(&env, series_id);
    }

    // ── Admin: fee and config ─────────────────────────────────────────────────

    pub fn set_fee_rate(env: Env, new_bps: i128) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        if new_bps > MAX_FEE_RATE_BPS || new_bps < 0 {
            panic_with_error!(env, Error::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::FeeRateBps, &new_bps);
        events::fee_rate_updated(&env, new_bps);
    }

    pub fn set_fee_rate_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_bps: i128,
    ) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        if new_bps > MAX_FEE_RATE_BPS || new_bps < 0 {
            panic_with_error!(env, Error::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::FeeRateBps, &new_bps);
        events::fee_rate_updated(&env, new_bps);
    }

    pub fn set_max_active_series(env: Env, cap: u32) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        if cap == 0 || cap > 500 {
            panic_with_error!(env, Error::InvalidActiveSeriesCap);
        }
        env.storage().instance().set(&DataKey::MaxActiveSeries, &cap);
    }

    // ── Admin: vault config ───────────────────────────────────────────────────

    /// Set the pinned trusted vault address.
    pub fn set_vault(env: Env, vault_contract: Address) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        let old: Option<Address> = env.storage().instance().get(&DataKey::Vault);
        env.storage().instance().set(&DataKey::Vault, &vault_contract);
        events::vault_updated(&env, old, vault_contract);
    }

    // ── Admin: pause / unpause ────────────────────────────────────────────────

    pub fn pause(env: Env) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        zenith_common::set_paused(&env, &DataKey::Paused, true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        zenith_common::set_paused(&env, &DataKey::Paused, false);
        events::unpaused(&env);
    }

    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Emergency,
            Error::Unauthorized,
        );
        zenith_common::set_paused(&env, &DataKey::Paused, true);
        events::paused(&env);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        zenith_common::set_paused(&env, &DataKey::Paused, false);
        events::unpaused(&env);
    }

    // ── Admin transfer ────────────────────────────────────────────────────────

    pub fn transfer_admin(env: Env, new_admin: Address) {
        crate::ttl::extend_instance(&env);
        let old = zenith_common::set_admin(&env, &DataKey::Admin, &new_admin);
        old.require_auth();
        events::admin_transferred(&env, old, new_admin);
    }

    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Critical,
            Error::Unauthorized,
        );
        let old = zenith_common::set_admin(&env, &DataKey::Admin, &new_admin);
        events::admin_transferred(&env, old, new_admin);
    }

    // ── Upgrade ───────────────────────────────────────────────────────────────

    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        crate::ttl::extend_instance(&env);
        get_admin(&env).require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    pub fn upgrade_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_wasm_hash: BytesN<32>,
    ) {
        crate::ttl::extend_instance(&env);
        use zenith_common::{ActionClass, Auth};
        zenith_common::require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Critical,
            Error::Unauthorized,
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    // ── Oracle: settlement price ──────────────────────────────────────────────

    /// Trusted-oracle-address flow: the oracle address set at initialize settles the series.
    pub fn set_settlement_price(env: Env, series_id: u64, price: i128) {
        crate::ttl::extend_instance(&env);
        get_oracle(&env).require_auth();
        let mut series: OptionSeries =
            crate::ttl::get_persistent(&env, &DataKey::Series(series_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
        if series.state == SeriesState::Settled {
            panic_with_error!(env, Error::AlreadySettled);
        }
        if series.state == SeriesState::Cancelled {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        let now = env.ledger().timestamp();
        if now <= series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        series.settlement_price = Some(price);
        series.state = SeriesState::Settled;
        decrement_active_count(&env, &series.underlying);
        crate::ttl::set_persistent(&env, &DataKey::Series(series_id), &series);
        events::settlement_price_set(&env, series_id, price);
    }

    /// Cross-contract oracle flow: reads the price from a live `price_oracle` deployment.
    pub fn set_settlement_price_from_oracle(env: Env, series_id: u64, oracle_contract: Address) {
        crate::ttl::extend_instance(&env);
        let series: OptionSeries =
            crate::ttl::get_persistent(&env, &DataKey::Series(series_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
        if series.state == SeriesState::Settled {
            panic_with_error!(env, Error::AlreadySettled);
        }
        if series.state == SeriesState::Cancelled {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        let now = env.ledger().timestamp();
        if now <= series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        // Cross-contract call to price_oracle.get_price(underlying)
        let price: Option<i128> = env.invoke_contract(
            &oracle_contract,
            &Symbol::new(&env, "get_price"),
            soroban_sdk::vec![&env, series.underlying.clone().into_val(&env)],
        );
        let price = price.unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));

        let mut series = series;
        series.settlement_price = Some(price);
        series.state = SeriesState::Settled;
        decrement_active_count(&env, &series.underlying);
        crate::ttl::set_persistent(&env, &DataKey::Series(series_id), &series);
        events::settlement_price_set(&env, series_id, price);
    }

    // ── Traders: buy ─────────────────────────────────────────────────────────

    /// Open a long (buyer) position.
    pub fn buy_option(
        env: Env,
        buyer: Address,
        series_id: u64,
        contracts: i128,
        max_premium: i128,
    ) -> u64 {
        crate::ttl::extend_instance(&env);
        buyer.require_auth();
        require_not_paused(&env);
        let mut series = require_active_series(&env, series_id);
        if contracts <= 0 {
            panic_with_error!(env, Error::ZeroContracts);
        }
        let total_premium = series.premium * contracts / PRICE_PRECISION;
        if total_premium > max_premium {
            panic_with_error!(env, Error::InsufficientPremium);
        }
        let fee_bps = fee_rate_bps(&env);
        let fee = calc_fee(total_premium, fee_bps);
        let net_premium = total_premium - fee;

        // Pull premium from buyer
        let collateral_token = get_collateral_token(&env);
        let contract_addr = env.current_contract_address();
        token::Client::new(&env, &collateral_token).transfer(
            &buyer,
            &contract_addr,
            &total_premium,
        );
        // Send fee to fee recipient
        if fee > 0 {
            token::Client::new(&env, &collateral_token).transfer(
                &contract_addr,
                &get_fee_recipient(&env),
                &fee,
            );
        }
        // Add net premium to pool
        let pool = get_premium_pool(&env);
        set_premium_pool(&env, pool + net_premium);

        // Update series open interest
        series.open_interest += contracts;
        crate::ttl::set_persistent(&env, &DataKey::Series(series_id), &series);

        // Track series escrow (refundable on cancellation)
        let escrow = series_escrow(&env, series_id);
        set_series_escrow(&env, series_id, escrow + net_premium);

        // Update stats
        let premiums: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalPremiumsCollected)
            .unwrap_or(0i128);
        env.storage()
            .instance()
            .set(&DataKey::TotalPremiumsCollected, &(premiums + total_premium));
        let oi: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalOpenInterest)
            .unwrap_or(0i128);
        env.storage()
            .instance()
            .set(&DataKey::TotalOpenInterest, &(oi + contracts));

        // Create position
        let pos_id = next_position_id(&env);
        let position = OptionPosition {
            position_id: pos_id,
            series_id,
            owner: buyer.clone(),
            side: PositionSide::Long,
            contracts,
            premium_paid: total_premium,
            fee_paid: fee,
            collateral_locked: 0,
            is_exercised: false,
            is_settled: false,
            opened_at: env.ledger().timestamp(),
        };
        crate::ttl::set_persistent(&env, &DataKey::Position(pos_id), &position);
        add_user_position(&env, &buyer, pos_id);

        events::option_bought(&env, buyer, pos_id, series_id, contracts, net_premium);
        pos_id
    }

    // ── Traders: write ────────────────────────────────────────────────────────

    /// Open a short (writer) position.
    pub fn write_option(
        env: Env,
        writer: Address,
        series_id: u64,
        contracts: i128,
        collateral_amount: i128,
    ) -> u64 {
        crate::ttl::extend_instance(&env);
        writer.require_auth();
        require_not_paused(&env);
        let mut series = require_active_series(&env, series_id);
        if contracts <= 0 {
            panic_with_error!(env, Error::ZeroContracts);
        }

        // Calculate required collateral (use strike as fallback price for calls)
        let price = series.strike_price;
        let req_collateral = required_collateral(&series.option_type, series.strike_price, price, contracts);
        if collateral_amount < req_collateral {
            panic_with_error!(env, Error::InsufficientCollateral);
        }

        // Calculate writer's net premium
        let gross_premium = series.premium * contracts / PRICE_PRECISION;
        let fee_bps = fee_rate_bps(&env);
        let fee = calc_fee(gross_premium, fee_bps);
        let writer_net = gross_premium - fee;

        // Check pool has enough
        let pool = get_premium_pool(&env);
        if pool < writer_net {
            panic_with_error!(env, Error::InsufficientPremiumPool);
        }

        // Pull collateral from writer
        let collateral_token = get_collateral_token(&env);
        let contract_addr = env.current_contract_address();
        token::Client::new(&env, &collateral_token).transfer(
            &writer,
            &contract_addr,
            &req_collateral,
        );

        // Pay writer from pool
        set_premium_pool(&env, pool - writer_net);
        if writer_net > 0 {
            token::Client::new(&env, &collateral_token).transfer(
                &contract_addr,
                &writer,
                &writer_net,
            );
        }

        // Update series open interest
        series.open_interest += contracts;
        crate::ttl::set_persistent(&env, &DataKey::Series(series_id), &series);

        // Track series escrow (refundable collateral on cancellation)
        let escrow = series_escrow(&env, series_id);
        set_series_escrow(&env, series_id, escrow + req_collateral);

        // Update stats
        let oi: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalOpenInterest)
            .unwrap_or(0i128);
        env.storage()
            .instance()
            .set(&DataKey::TotalOpenInterest, &(oi + contracts));

        // Create position
        let pos_id = next_position_id(&env);
        let position = OptionPosition {
            position_id: pos_id,
            series_id,
            owner: writer.clone(),
            side: PositionSide::Short,
            contracts,
            premium_paid: writer_net,
            fee_paid: 0,
            collateral_locked: req_collateral,
            is_exercised: false,
            is_settled: false,
            opened_at: env.ledger().timestamp(),
        };
        crate::ttl::set_persistent(&env, &DataKey::Position(pos_id), &position);
        add_user_position(&env, &writer, pos_id);

        events::option_written(&env, writer, pos_id, series_id, contracts, writer_net, req_collateral);
        pos_id
    }

    // ── Traders: exercise ────────────────────────────────────────────────────

    /// Claim payout for an in-the-money long position after settlement.
    pub fn exercise(env: Env, owner: Address, position_id: u64) -> i128 {
        crate::ttl::extend_instance(&env);
        owner.require_auth();
        Self::exercise_inner(&env, &owner, position_id)
    }

    fn exercise_inner(env: &Env, owner: &Address, position_id: u64) -> i128 {
        let mut position: OptionPosition =
            crate::ttl::get_persistent(env, &DataKey::Position(position_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::PositionNotFound));
        if position.owner != *owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.side != PositionSide::Long {
            panic_with_error!(env, Error::WrongSide);
        }
        if position.is_exercised {
            panic_with_error!(env, Error::AlreadyExercised);
        }

        let series: OptionSeries =
            crate::ttl::get_persistent(env, &DataKey::Series(position.series_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));

        let now = env.ledger().timestamp();
        if now <= series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        if series.state != SeriesState::Settled {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));

        let payout = calc_payout(&series.option_type, series.strike_price, settlement_price, position.contracts);
        if payout == 0 {
            panic_with_error!(env, Error::NotInTheMoney);
        }

        position.is_exercised = true;
        crate::ttl::set_persistent(env, &DataKey::Position(position_id), &position);

        // Transfer payout
        let collateral_token: Address = env.storage().instance().get(&DataKey::CollateralToken).unwrap();
        token::Client::new(env, &collateral_token).transfer(
            &env.current_contract_address(),
            owner,
            &payout,
        );

        events::option_exercised(env, owner.clone(), position_id, settlement_price, payout);
        payout
    }

    /// Batched exercise for up to `MAX_BATCH_SIZE` positions.
    pub fn exercise_batch(env: Env, owner: Address, position_ids: Vec<u64>) -> i128 {
        crate::ttl::extend_instance(&env);
        owner.require_auth();
        let n = position_ids.len();
        if n == 0 || n > MAX_BATCH_SIZE {
            panic_with_error!(env, Error::InvalidBatchSize);
        }
        let mut total = 0i128;
        for id in position_ids.iter() {
            total += Self::exercise_inner(&env, &owner, id);
        }
        total
    }

    // ── Traders: reclaim collateral ───────────────────────────────────────────

    /// Return locked collateral minus max loss to a writer after settlement.
    pub fn reclaim_collateral(env: Env, writer: Address, position_id: u64) -> i128 {
        crate::ttl::extend_instance(&env);
        writer.require_auth();
        Self::reclaim_inner(&env, &writer, position_id)
    }

    fn reclaim_inner(env: &Env, writer: &Address, position_id: u64) -> i128 {
        let mut position: OptionPosition =
            crate::ttl::get_persistent(env, &DataKey::Position(position_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::PositionNotFound));
        if position.owner != *writer {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.side != PositionSide::Short {
            panic_with_error!(env, Error::WrongSide);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }

        let series: OptionSeries =
            crate::ttl::get_persistent(env, &DataKey::Series(position.series_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
        if series.state != SeriesState::Settled {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));

        let max_loss = calc_payout(&series.option_type, series.strike_price, settlement_price, position.contracts);
        let reclaim = position.collateral_locked - max_loss;

        position.is_settled = true;
        crate::ttl::set_persistent(env, &DataKey::Position(position_id), &position);

        if reclaim > 0 {
            let collateral_token: Address = env.storage().instance().get(&DataKey::CollateralToken).unwrap();
            token::Client::new(env, &collateral_token).transfer(
                &env.current_contract_address(),
                writer,
                &reclaim,
            );
        }

        events::collateral_reclaimed(env, writer.clone(), position_id, reclaim);
        reclaim
    }

    pub fn reclaim_batch(env: Env, writer: Address, position_ids: Vec<u64>) -> i128 {
        crate::ttl::extend_instance(&env);
        writer.require_auth();
        let n = position_ids.len();
        if n == 0 || n > MAX_BATCH_SIZE {
            panic_with_error!(env, Error::InvalidBatchSize);
        }
        let mut total = 0i128;
        for id in position_ids.iter() {
            total += Self::reclaim_inner(&env, &writer, id);
        }
        total
    }

    // ── Traders: claim refund ─────────────────────────────────────────────────

    /// On a Cancelled series: buyers get premium net of fee back; writers get collateral back.
    pub fn claim_refund(env: Env, owner: Address, position_id: u64) {
        crate::ttl::extend_instance(&env);
        owner.require_auth();
        Self::claim_refund_inner(&env, &owner, position_id);
    }

    fn claim_refund_inner(env: &Env, owner: &Address, position_id: u64) {
        let mut position: OptionPosition =
            crate::ttl::get_persistent(env, &DataKey::Position(position_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::PositionNotFound));
        if position.owner != *owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }

        let series: OptionSeries =
            crate::ttl::get_persistent(env, &DataKey::Series(position.series_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
        if series.state != SeriesState::Cancelled {
            panic_with_error!(env, Error::SeriesNotCancelled);
        }

        let refund = match position.side {
            PositionSide::Long => position.premium_paid - position.fee_paid,
            PositionSide::Short => position.collateral_locked,
        };

        position.is_settled = true;
        crate::ttl::set_persistent(env, &DataKey::Position(position_id), &position);

        // Debit series escrow
        let escrow = series_escrow(env, position.series_id);
        set_series_escrow(env, position.series_id, escrow.saturating_sub(refund));

        if refund > 0 {
            let collateral_token: Address = env.storage().instance().get(&DataKey::CollateralToken).unwrap();
            token::Client::new(env, &collateral_token).transfer(
                &env.current_contract_address(),
                owner,
                &refund,
            );
        }

        events::refund_claimed(env, owner.clone(), position_id, refund);
    }

    // ── Vault integration ─────────────────────────────────────────────────────

    /// Move a cancelled series' outstanding refund liability into a vault.
    pub fn escrow_series_to_vault(env: Env, vault_contract: Address, series_id: u64) {
        crate::ttl::extend_instance(&env);
        // Check vault is the pinned one
        let pinned: Option<Address> = env.storage().instance().get(&DataKey::Vault);
        if pinned.as_ref() != Some(&vault_contract) {
            panic_with_error!(env, Error::Unauthorized);
        }
        let series: OptionSeries =
            crate::ttl::get_persistent(&env, &DataKey::Series(series_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
        if series.state != SeriesState::Cancelled {
            panic_with_error!(env, Error::SeriesNotCancelled);
        }
        let amount = series_escrow(&env, series_id);
        if amount == 0 {
            panic_with_error!(env, Error::NothingToEscrow);
        }
        set_series_escrow(&env, series_id, 0);

        // Record which vault holds escrow for this series
        crate::ttl::set_persistent(&env, &DataKey::SeriesVault(series_id), &vault_contract);

        // Deposit into vault
        let collateral_token = get_collateral_token(&env);
        let contract_addr = env.current_contract_address();
        // Approve vault to pull from us, then call vault.deposit
        token::Client::new(&env, &collateral_token).transfer(
            &contract_addr,
            &vault_contract,
            &amount,
        );
        // Call vault's deposit_legacy for series_id
        env.invoke_contract::<i128>(
            &vault_contract,
            &Symbol::new(&env, "deposit_legacy"),
            soroban_sdk::vec![
                &env,
                contract_addr.into_val(&env),
                series_id.into_val(&env),
                amount.into_val(&env),
            ],
        );

        events::series_escrowed_to_vault(&env, series_id, amount);
    }

    /// Claim refund from vault instead of from options_market's own balance.
    pub fn claim_refund_from_vault(
        env: Env,
        vault_contract: Address,
        owner: Address,
        position_id: u64,
    ) {
        crate::ttl::extend_instance(&env);
        owner.require_auth();

        let mut position: OptionPosition =
            crate::ttl::get_persistent(&env, &DataKey::Position(position_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::PositionNotFound));
        if position.owner != owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }

        let series: OptionSeries =
            crate::ttl::get_persistent(&env, &DataKey::Series(position.series_id))
                .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
        if series.state != SeriesState::Cancelled {
            panic_with_error!(env, Error::SeriesNotCancelled);
        }

        // Verify vault_contract is the one recorded for this series
        let recorded_vault: Option<Address> =
            crate::ttl::get_persistent(&env, &DataKey::SeriesVault(position.series_id));
        if recorded_vault.as_ref() != Some(&vault_contract) {
            panic_with_error!(env, Error::Unauthorized);
        }

        let refund = match position.side {
            PositionSide::Long => position.premium_paid - position.fee_paid,
            PositionSide::Short => position.collateral_locked,
        };

        position.is_settled = true;
        crate::ttl::set_persistent(&env, &DataKey::Position(position_id), &position);

        if refund > 0 {
            env.invoke_contract::<()>(
                &vault_contract,
                &Symbol::new(&env, "withdraw_legacy"),
                soroban_sdk::vec![
                    &env,
                    position.series_id.into_val(&env),
                    owner.clone().into_val(&env),
                    refund.into_val(&env),
                ],
            );
        }

        events::refund_claimed(&env, owner, position_id, refund);
    }

    // ── Keeper ────────────────────────────────────────────────────────────────

    /// Extend TTL for the instance and any named persistent keys.
    pub fn bump(env: Env, _keys: Vec<DataKey>) {
        crate::ttl::extend_instance(&env);
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn get_series(env: Env, series_id: u64) -> Option<OptionSeries> {
        crate::ttl::extend_instance(&env);
        crate::ttl::get_persistent(&env, &DataKey::Series(series_id))
    }

    pub fn get_position(env: Env, position_id: u64) -> Option<OptionPosition> {
        crate::ttl::extend_instance(&env);
        crate::ttl::get_persistent(&env, &DataKey::Position(position_id))
    }

    pub fn get_user_positions(env: Env, owner: Address) -> Vec<u64> {
        crate::ttl::extend_instance(&env);
        crate::ttl::get_persistent(&env, &DataKey::UserPositions(owner))
            .unwrap_or(Vec::new(&env))
    }

    /// Returns `(total_premiums_collected, total_open_interest, series_count)`.
    pub fn get_stats(env: Env) -> (i128, i128, u64) {
        crate::ttl::extend_instance(&env);
        let premiums = env
            .storage()
            .instance()
            .get(&DataKey::TotalPremiumsCollected)
            .unwrap_or(0i128);
        let oi = env
            .storage()
            .instance()
            .get(&DataKey::TotalOpenInterest)
            .unwrap_or(0i128);
        let count = series_counter(&env);
        (premiums, oi, count)
    }

    pub fn get_premium_pool(env: Env) -> i128 {
        crate::ttl::extend_instance(&env);
        get_premium_pool(&env)
    }

    pub fn get_admin(env: Env) -> Address {
        crate::ttl::extend_instance(&env);
        get_admin(&env)
    }

    pub fn is_paused(env: Env) -> bool {
        crate::ttl::extend_instance(&env);
        zenith_common::is_paused(&env, &DataKey::Paused)
    }

    pub fn get_fee_rate(env: Env) -> i128 {
        crate::ttl::extend_instance(&env);
        fee_rate_bps(&env)
    }

    pub fn get_active_series_count(env: Env, underlying: Symbol) -> u32 {
        crate::ttl::extend_instance(&env);
        active_series_count(&env, &underlying)
    }

    pub fn get_max_active_series(env: Env) -> u32 {
        crate::ttl::extend_instance(&env);
        max_active_series(&env)
    }

    pub fn get_series_count_for_underlying(env: Env, underlying: Symbol) -> u64 {
        crate::ttl::extend_instance(&env);
        series_count_for_underlying(&env, &underlying)
    }

    pub fn get_series_escrow(env: Env, series_id: u64) -> i128 {
        crate::ttl::extend_instance(&env);
        series_escrow(&env, series_id)
    }

    pub fn get_vault(env: Env) -> Option<Address> {
        crate::ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Vault)
    }

    pub fn get_series_vault(env: Env, series_id: u64) -> Option<Address> {
        crate::ttl::extend_instance(&env);
        crate::ttl::get_persistent(&env, &DataKey::SeriesVault(series_id))
    }
}
