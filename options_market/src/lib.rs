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
mod migrations;
mod multisig_client;
mod price_oracle_client;
mod storage;
mod ttl;
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

#[cfg(test)]
mod test;

#[cfg(test)]
mod test_adversaries;
#[cfg(test)]
mod test_resources;

// ─── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct OptionsMarket;

#[contractimpl]
impl OptionsMarket {
    // ─── Admin ────────────────────────────────────────────────────────────

    /// One-time initialization. Panics with `AlreadyInitialized` if called
    /// twice.
    pub fn initialize(
        env: Env,
        admin: Address,
        oracle: Address,
        collateral_token: Address,
        fee_recipient: Address,
    ) {
        ttl::extend_instance(&env);
        if storage::is_initialized(&env) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        storage::initialize(&env, &admin, &oracle, &collateral_token, &fee_recipient);
        migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    /// Returns the current admin address.
    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        storage::admin(&env)
    }

    /// Hands admin control to `new_admin`. Requires the current admin's
    /// signature.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        ttl::extend_instance(&env);
        let old = storage::admin(&env);
        old.require_auth();
        storage::set_admin(&env, &new_admin);
        events::admin_transferred(&env, old, new_admin);
    }

    /// Multisig-gated alternative to `transfer_admin`.
    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        ttl::extend_instance(&env);
        use zenith_common::{require_admin_or_multisig, ActionClass, Auth};
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Critical,
            Error::Unauthorized,
        );
        let old = storage::admin(&env);
        storage::set_admin(&env, &new_admin);
        events::admin_transferred(&env, old, new_admin);
    }

    // ─── Pause ────────────────────────────────────────────────────────────

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        storage::is_paused(&env)
    }

    pub fn pause(env: Env) {
        ttl::extend_instance(&env);
        storage::admin(&env).require_auth();
        storage::set_paused(&env, true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        ttl::extend_instance(&env);
        storage::admin(&env).require_auth();
        storage::set_paused(&env, false);
        events::unpaused(&env);
    }

    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        ttl::extend_instance(&env);
        use zenith_common::{require_admin_or_multisig, ActionClass, Auth};
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Emergency,
            Error::Unauthorized,
        );
        storage::set_paused(&env, true);
        events::paused(&env);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        ttl::extend_instance(&env);
        use zenith_common::{require_admin_or_multisig, ActionClass, Auth};
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        storage::set_paused(&env, false);
        events::unpaused(&env);
    }

    // ─── Fee rate ─────────────────────────────────────────────────────────

    pub fn get_fee_rate(env: Env) -> i128 {
        ttl::extend_instance(&env);
        storage::fee_rate_bps(&env)
    }

    pub fn set_fee_rate(env: Env, new_bps: i128) {
        ttl::extend_instance(&env);
        storage::admin(&env).require_auth();
        if new_bps < 0 || new_bps > MAX_FEE_RATE_BPS {
            panic_with_error!(&env, Error::InvalidFeeRate);
        }
        storage::set_fee_rate_bps(&env, new_bps);
        events::fee_rate_updated(&env, new_bps);
    }

    pub fn set_fee_rate_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_bps: i128,
    ) {
        ttl::extend_instance(&env);
        use zenith_common::{require_admin_or_multisig, ActionClass, Auth};
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        if new_bps < 0 || new_bps > MAX_FEE_RATE_BPS {
            panic_with_error!(&env, Error::InvalidFeeRate);
        }
        storage::set_fee_rate_bps(&env, new_bps);
        events::fee_rate_updated(&env, new_bps);
    }

    // ─── Premium pool ─────────────────────────────────────────────────────

    pub fn get_premium_pool(env: Env) -> i128 {
        ttl::extend_instance(&env);
        storage::premium_pool(&env)
    }

    // ─── Vault ────────────────────────────────────────────────────────────

    /// Returns the pinned vault address, if one has been set.
    pub fn get_vault(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        storage::vault(&env)
    }

    /// Pins a trusted vault contract. Admin only.
    pub fn set_vault(env: Env, vault_contract: Address) {
        ttl::extend_instance(&env);
        storage::admin(&env).require_auth();
        let old = storage::vault(&env);
        storage::set_vault(&env, &vault_contract);
        events::vault_updated(&env, old, vault_contract);
    }

    /// Returns the vault holding escrow for `series_id`, if any.
    pub fn get_series_vault(env: Env, series_id: u64) -> Option<Address> {
        ttl::extend_instance(&env);
        storage::series_vault(&env, series_id)
    }

    // ─── Upgrade ──────────────────────────────────────────────────────────

    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        storage::admin(&env).require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    pub fn upgrade_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_wasm_hash: BytesN<32>,
    ) {
        ttl::extend_instance(&env);
        use zenith_common::{require_admin_or_multisig, ActionClass, Auth};
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Critical,
            Error::Unauthorized,
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    // ─── Series management ────────────────────────────────────────────────

    pub fn create_series(
        env: Env,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
        premium: i128,
        implied_vol: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        storage::admin(&env).require_auth();
        Self::create_series_inner(
            &env,
            underlying,
            option_type,
            strike_price,
            expiry,
            premium,
            implied_vol,
        )
    }

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
        ttl::extend_instance(&env);
        require_not_paused(&env);
        use zenith_common::{require_admin_or_multisig, ActionClass, Auth};
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        Self::create_series_inner(
            &env,
            underlying,
            option_type,
            strike_price,
            expiry,
            premium,
            implied_vol,
        )
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
        let now = env.ledger().timestamp();
        // expiry must be at least 1 hour out
        if expiry <= now + 3_600 {
            panic_with_error!(env, Error::ExpiryTooSoon);
        }
        if strike_price <= 0 || premium < 0 || implied_vol < 0 {
            panic_with_error!(env, Error::InvalidSeriesParams);
        }
        if premium == 0 {
            panic_with_error!(env, Error::InvalidSeriesParams);
        }

        let count = storage::series_count_for_underlying(env, &underlying);
        if count >= MAX_SERIES_PER_UNDERLYING {
            panic_with_error!(env, Error::TooManySeriesForUnderlying);
        }

        let series_id = storage::next_series_id(env);
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
        storage::set_series(env, series_id, &series);
        storage::increment_series_count_for_underlying(env, &underlying);

        // Update stats counter
        let sc = storage::series_counter(env);
        let _ = sc; // already incremented inside next_series_id

        events::series_created(env, series_id, strike_price, expiry, premium);
        series_id
    }

    pub fn update_premium(env: Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
        ttl::extend_instance(&env);
        storage::admin(&env).require_auth();
        Self::update_premium_inner(&env, series_id, new_premium, new_implied_vol);
    }

    pub fn update_premium_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        series_id: u64,
        new_premium: i128,
        new_implied_vol: i128,
    ) {
        ttl::extend_instance(&env);
        use zenith_common::{require_admin_or_multisig, ActionClass, Auth};
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        Self::update_premium_inner(&env, series_id, new_premium, new_implied_vol);
    }

    fn update_premium_inner(env: &Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
        if new_premium <= 0 {
            panic_with_error!(env, Error::InvalidSeriesParams);
        }
        let mut series = require_active_series(env, series_id);
        series.premium = new_premium;
        series.implied_vol = new_implied_vol;
        storage::set_series(env, series_id, &series);
        events::premium_updated(env, series_id, new_premium, new_implied_vol);
    }

    pub fn cancel_series(env: Env, series_id: u64) {
        ttl::extend_instance(&env);
        storage::admin(&env).require_auth();
        Self::cancel_series_inner(&env, series_id);
    }

    pub fn cancel_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        series_id: u64,
    ) {
        ttl::extend_instance(&env);
        use zenith_common::{require_admin_or_multisig, ActionClass, Auth};
        require_admin_or_multisig(
            &env,
            &DataKey::Admin,
            &Auth::Multisig(multisig_contract, action_id),
            ActionClass::Standard,
            Error::Unauthorized,
        );
        Self::cancel_series_inner(&env, series_id);
    }

    fn cancel_series_inner(env: &Env, series_id: u64) {
        let mut series = require_active_series(env, series_id);
        series.state = SeriesState::Cancelled;
        storage::set_series(env, series_id, &series);
        events::series_cancelled(env, series_id);
    }

    // ─── Oracle / settlement ──────────────────────────────────────────────

    pub fn set_settlement_price(env: Env, series_id: u64, price: i128) {
        ttl::extend_instance(&env);
        storage::oracle(&env).require_auth();
        let mut series = storage::require_series(&env, series_id);
        if series.state == SeriesState::Settled {
            panic_with_error!(&env, Error::AlreadySettled);
        }
        if series.state == SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotActive);
        }
        let now = env.ledger().timestamp();
        if now <= series.expiry {
            panic_with_error!(&env, Error::SeriesNotExpired);
        }
        series.settlement_price = Some(price);
        series.state = SeriesState::Settled;
        storage::set_series(&env, series_id, &series);
        storage::set_underlying_price(&env, &series.underlying, price);
        events::settlement_price_set(&env, series_id, price);
    }

    pub fn set_settlement_price_from_oracle(
        env: Env,
        series_id: u64,
        oracle_contract: Address,
    ) {
        ttl::extend_instance(&env);
        let mut series = storage::require_series(&env, series_id);
        if series.state == SeriesState::Settled {
            panic_with_error!(&env, Error::AlreadySettled);
        }
        if series.state == SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotActive);
        }
        let now = env.ledger().timestamp();
        if now <= series.expiry {
            panic_with_error!(&env, Error::SeriesNotExpired);
        }
        let client = price_oracle_client::Client::new(&env, &oracle_contract);
        let price = client
            .get_price(&series.underlying)
            .unwrap_or_else(|| panic_with_error!(&env, Error::PriceNotSet));
        series.settlement_price = Some(price);
        series.state = SeriesState::Settled;
        storage::set_series(&env, series_id, &series);
        storage::set_underlying_price(&env, &series.underlying, price);
        events::settlement_price_set(&env, series_id, price);
    }

    // ─── buy_option ───────────────────────────────────────────────────────

    pub fn buy_option(
        env: Env,
        buyer: Address,
        series_id: u64,
        contracts: i128,
        max_premium: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        buyer.require_auth();
        Self::buy_option_inner(&env, buyer, series_id, contracts, max_premium)
    }

    fn buy_option_inner(
        env: &Env,
        buyer: Address,
        series_id: u64,
        contracts: i128,
        max_premium: i128,
    ) -> u64 {
        if contracts <= 0 {
            panic_with_error!(env, Error::ZeroContracts);
        }
        let mut series = require_active_series(env, series_id);

        let total_premium = series
            .premium
            .checked_mul(contracts)
            .expect("premium overflow")
            / PRICE_PRECISION;

        if total_premium > max_premium {
            panic_with_error!(env, Error::InsufficientPremium);
        }

        let fee = calc_fee(total_premium, fee_rate_bps(env));
        let net_premium = total_premium - fee;

        // Pull full premium from buyer
        let token_client = token::Client::new(env, &storage::collateral_token(env));
        token_client.transfer(&buyer, &env.current_contract_address(), &total_premium);

        // Pay protocol fee to fee recipient
        if fee > 0 {
            token_client.transfer(
                &env.current_contract_address(),
                &storage::fee_recipient(env),
                &fee,
            );
        }

        // Accumulate into premium pool
        let pool = storage::premium_pool(env) + net_premium;
        storage::set_premium_pool(env, pool);

        // Accumulators
        let premiums = storage::total_premiums_collected(env) + total_premium;
        storage::set_total_premiums_collected(env, premiums);

        // Track series escrow (refundable if cancelled)
        storage::increase_series_escrow(env, series_id, net_premium);

        series.open_interest += contracts;
        storage::set_series(env, series_id, &series);

        let pos_id = next_position_id(env);
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
        storage::set_position(env, pos_id, &position);
        add_user_position(env, &buyer, pos_id);

        // Update open interest accumulator
        let oi = storage::total_open_interest(env) + contracts;
        storage::set_total_open_interest(env, oi);

        events::option_bought(env, buyer, pos_id, series_id, contracts, total_premium);
        pos_id
    }

    pub fn buy_batch(
        env: Env,
        buyer: Address,
        orders: Vec<(u64, i128, i128)>,
    ) -> Vec<u64> {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        buyer.require_auth();
        if orders.is_empty() || orders.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut result = Vec::new(&env);
        for (series_id, contracts, max_premium) in orders.iter() {
            let pos_id = Self::buy_option_inner(&env, buyer.clone(), series_id, contracts, max_premium);
            result.push_back(pos_id);
        }
        result
    }

    // ─── write_option ─────────────────────────────────────────────────────

    pub fn write_option(
        env: Env,
        writer: Address,
        series_id: u64,
        contracts: i128,
        collateral_amount: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        writer.require_auth();
        Self::write_option_inner(&env, writer, series_id, contracts, collateral_amount, 0)
    }

    fn write_option_inner(
        env: &Env,
        writer: Address,
        series_id: u64,
        contracts: i128,
        collateral_amount: i128,
        min_premium: i128,
    ) -> u64 {
        if contracts <= 0 {
            panic_with_error!(env, Error::ZeroContracts);
        }
        let mut series = require_active_series(env, series_id);

        // Determine required collateral
        let required_collateral = if series.option_type == OptionType::Call {
            // Use underlying price if available, else strike as fallback
            let price = storage::underlying_price(env, &series.underlying)
                .unwrap_or(series.strike_price);
            price
                .checked_mul(contracts)
                .expect("collateral overflow")
                / PRICE_PRECISION
        } else {
            // Put: 110% of strike
            series
                .strike_price
                .checked_mul(contracts)
                .expect("collateral overflow")
                / PRICE_PRECISION
                * MIN_COLLATERAL_RATIO
                / RATE_PRECISION
        };

        if collateral_amount < required_collateral {
            panic_with_error!(env, Error::InsufficientCollateral);
        }

        // Calculate writer premium payout (net of fee) from pool
        let gross_premium = series
            .premium
            .checked_mul(contracts)
            .expect("premium overflow")
            / PRICE_PRECISION;
        let fee = calc_fee(gross_premium, fee_rate_bps(env));
        let writer_premium = gross_premium - fee;

        if writer_premium > storage::premium_pool(env) {
            panic_with_error!(env, Error::InsufficientPremiumPool);
        }

        if min_premium > 0 && writer_premium < min_premium {
            panic_with_error!(env, Error::PremiumBelowMinimum);
        }

        let token_client = token::Client::new(env, &storage::collateral_token(env));

        // Pull collateral from writer
        token_client.transfer(&writer, &env.current_contract_address(), &required_collateral);

        // Pay writer from premium pool
        if writer_premium > 0 {
            token_client.transfer(&env.current_contract_address(), &writer, &writer_premium);
        }

        // Pay fee to fee recipient
        if fee > 0 {
            token_client.transfer(
                &env.current_contract_address(),
                &storage::fee_recipient(env),
                &fee,
            );
        }

        // Deduct from pool
        let pool = storage::premium_pool(env) - writer_premium - fee;
        storage::set_premium_pool(env, pool.max(0));

        // Track series escrow (collateral refundable if cancelled)
        storage::increase_series_escrow(env, series_id, required_collateral);

        series.open_interest += contracts;
        storage::set_series(env, series_id, &series);

        let pos_id = next_position_id(env);
        let position = OptionPosition {
            position_id: pos_id,
            series_id,
            owner: writer.clone(),
            side: PositionSide::Short,
            contracts,
            premium_paid: writer_premium,
            fee_paid: fee,
            collateral_locked: required_collateral,
            is_exercised: false,
            is_settled: false,
            opened_at: env.ledger().timestamp(),
        };
        storage::set_position(env, pos_id, &position);
        add_user_position(env, &writer, pos_id);

        let oi = storage::total_open_interest(env) + contracts;
        storage::set_total_open_interest(env, oi);

        events::option_written(
            env,
            writer,
            pos_id,
            series_id,
            contracts,
            writer_premium,
            required_collateral,
        );
        pos_id
    }

    pub fn write_batch(
        env: Env,
        writer: Address,
        orders: Vec<(u64, i128, i128)>,
    ) -> Vec<u64> {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        writer.require_auth();
        if orders.is_empty() || orders.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut result = Vec::new(&env);
        for (series_id, contracts, collateral_amount) in orders.iter() {
            let pos_id = Self::write_option_inner(&env, writer.clone(), series_id, contracts, collateral_amount, 0);
            result.push_back(pos_id);
        }
        result
    }

    // ─── exercise ─────────────────────────────────────────────────────────

    pub fn exercise(env: Env, owner: Address, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        Self::exercise_inner(&env, owner, position_id)
    }

    fn exercise_inner(env: &Env, owner: Address, position_id: u64) -> i128 {
        let mut position = storage::require_position(env, position_id);
        if position.owner != owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.side != PositionSide::Long {
            panic_with_error!(env, Error::WrongSide);
        }
        if position.is_exercised {
            panic_with_error!(env, Error::AlreadyExercised);
        }
        let series = storage::require_series(env, position.series_id);
        let now = env.ledger().timestamp();
        if now <= series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));

        let payout = calc_payout(
            series.option_type == OptionType::Call,
            series.strike_price,
            settlement_price,
            position.contracts,
        );
        if payout == 0 {
            panic_with_error!(env, Error::NotInTheMoney);
        }

        position.is_exercised = true;
        storage::set_position(env, position_id, &position);

        let token_client = token::Client::new(env, &storage::collateral_token(env));
        token_client.transfer(&env.current_contract_address(), &owner, &payout);

        events::option_exercised(env, owner, position_id, settlement_price, payout);
        payout
    }

    pub fn exercise_batch(env: Env, owner: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        if position_ids.is_empty() || position_ids.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut total: i128 = 0;
        for id in position_ids.iter() {
            total += Self::exercise_inner(&env, owner.clone(), id);
        }
        total
    }

    // ─── reclaim_collateral ───────────────────────────────────────────────

    pub fn reclaim_collateral(env: Env, writer: Address, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        writer.require_auth();
        Self::reclaim_inner(&env, writer, position_id)
    }

    fn reclaim_inner(env: &Env, writer: Address, position_id: u64) -> i128 {
        let mut position = storage::require_position(env, position_id);
        if position.owner != writer {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.side != PositionSide::Short {
            panic_with_error!(env, Error::WrongSide);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }
        let series = storage::require_series(env, position.series_id);
        if series.state != SeriesState::Settled {
            panic_with_error!(env, Error::SeriesNotActive);
        }

        let settlement_price = series.settlement_price.unwrap();
        let max_loss = calc_payout(
            series.option_type == OptionType::Call,
            series.strike_price,
            settlement_price,
            position.contracts,
        );
        let reclaim = (position.collateral_locked - max_loss).max(0);

        position.is_settled = true;
        storage::set_position(env, position_id, &position);

        if reclaim > 0 {
            let token_client = token::Client::new(env, &storage::collateral_token(env));
            token_client.transfer(&env.current_contract_address(), &writer, &reclaim);
        }

        events::collateral_reclaimed(env, writer, position_id, reclaim);
        reclaim
    }

    pub fn reclaim_batch(env: Env, writer: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        writer.require_auth();
        if position_ids.is_empty() || position_ids.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut total: i128 = 0;
        for id in position_ids.iter() {
            total += Self::reclaim_inner(&env, writer.clone(), id);
        }
        total
    }

    // ─── claim_refund ─────────────────────────────────────────────────────

    pub fn claim_refund(env: Env, owner: Address, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        let (refund, _) = Self::refund_amount(&env, owner.clone(), position_id);
        let mut position = storage::require_position(&env, position_id);
        position.is_settled = true;
        storage::set_position(&env, position_id, &position);
        storage::decrease_series_escrow(&env, position.series_id, refund);

        if refund > 0 {
            let token_client = token::Client::new(&env, &storage::collateral_token(&env));
            token_client.transfer(&env.current_contract_address(), &owner, &refund);
        }
        events::refund_claimed(&env, owner, position_id, refund);
        refund
    }

    pub fn claim_refund_from_vault(
        env: Env,
        vault_contract: Address,
        owner: Address,
        position_id: u64,
    ) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        let (refund, series_id) = Self::refund_amount(&env, owner.clone(), position_id);
        let mut position = storage::require_position(&env, position_id);
        position.is_settled = true;
        storage::set_position(&env, position_id, &position);
        storage::decrease_series_escrow(&env, series_id, refund);

        if refund > 0 {
            let client = vault_client::Client::new(&env, &vault_contract);
            client.withdraw(&series_id, &owner, &refund);
        }
        events::refund_claimed(&env, owner, position_id, refund);
        refund
    }

    fn refund_amount(env: &Env, owner: Address, position_id: u64) -> (i128, u64) {
        let position = storage::require_position(env, position_id);
        if position.owner != owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }
        let series = storage::require_series(env, position.series_id);
        if series.state != SeriesState::Cancelled {
            panic_with_error!(env, Error::SeriesNotCancelled);
        }
        let refund = if position.side == PositionSide::Long {
            // Premium net of fee already sent to fee_recipient at buy time.
            position.premium_paid - position.fee_paid
        } else {
            position.collateral_locked
        };
        (refund, position.series_id)
    }

    // ─── escrow_series_to_vault ───────────────────────────────────────────

    pub fn escrow_series_to_vault(env: Env, vault_contract: Address, series_id: u64) {
        ttl::extend_instance(&env);
        let series = storage::require_series(&env, series_id);
        if series.state != SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotCancelled);
        }
        let amount = storage::take_series_escrow(&env, series_id);
        if amount == 0 {
            panic_with_error!(&env, Error::NothingToEscrow);
        }
        // Transfer from this contract's balance into vault tagged by series_id
        let token_client = token::Client::new(&env, &storage::collateral_token(&env));
        let vault_client = vault_client::Client::new(&env, &vault_contract);
        token_client.transfer(&env.current_contract_address(), &vault_contract, &amount);
        vault_client.deposit(&env.current_contract_address(), &series_id, &amount);

        storage::set_series_vault(&env, series_id, &vault_contract);
        events::series_escrowed_to_vault(&env, series_id, amount);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    pub fn get_series(env: Env, series_id: u64) -> Option<OptionSeries> {
        ttl::extend_instance(&env);
        storage::get_series(&env, series_id)
    }

    pub fn get_position(env: Env, position_id: u64) -> Option<OptionPosition> {
        ttl::extend_instance(&env);
        storage::get_position(&env, position_id)
    }

    pub fn get_user_positions(env: Env, user: Address) -> Vec<u64> {
        ttl::extend_instance(&env);
        storage::user_positions(&env, &user)
    }

    pub fn get_series_escrow(env: Env, series_id: u64) -> i128 {
        ttl::extend_instance(&env);
        storage::series_escrow(&env, series_id)
    }

    /// Returns `(total_premiums_collected, total_open_interest, series_counter)`.
    pub fn get_stats(env: Env) -> (i128, i128, u64) {
        ttl::extend_instance(&env);
        (
            storage::total_premiums_collected(&env),
            storage::total_open_interest(&env),
            storage::series_counter(&env),
        )
    }
}
