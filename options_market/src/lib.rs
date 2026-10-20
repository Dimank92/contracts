#![no_std]

//! Zenith Protocol — Decentralized Options Market on Stellar Soroban
//!
//! European-style put/call options on XLM, BTC, ETH, and SOL.
//! Premium is set by the admin (computed off-chain via Black-Scholes).
//! Writers lock collateral; buyers pay premium. Settlement at expiry via oracle.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, token, Address, BytesN, Env, IntoVal, Symbol, Val,
    Vec,
};

mod call_collateral;
mod error;
mod events;
mod math;
mod migrations;
mod multisig_client;
mod price_oracle_client;
mod rounding_policy;
mod settlement_fallback;
mod storage;
mod ttl;
mod types;
mod vault_client;
mod params_client;

use error::Error;
use math::{
    calc_fee, calc_payout, required_collateral, DEFAULT_FEE_RATE_BPS, MAX_BATCH_SIZE,
    MAX_FEE_RATE_BPS, MAX_SERIES_PER_UNDERLYING, PRICE_PRECISION,
};
use storage::{
    add_user_position, fee_rate_bps, get_admin, get_collateral_token, get_fee_recipient,
    get_oracle, get_position, get_series, get_series_vault, get_underlying_price, get_user_positions,
    get_vault, is_paused as storage_is_paused, next_position_id, next_series_id, premium_pool, require_active_series,
    require_not_paused, require_position, require_series, series_count_for_underlying,
    series_counter, series_escrow, set_admin, set_fee_rate_bps, set_position,
    set_premium_pool, set_series, set_series_count_for_underlying, set_series_escrow,
    set_series_vault, set_total_open_interest, set_total_premiums_collected, set_underlying_price,
    set_vault, total_open_interest, total_premiums_collected,
};
use types::{DataKey, OptionPosition, OptionSeries, OptionType, PositionSide, SeriesState};
use zenith_common::ActionClass;

#[cfg(test)]
mod test;

#[contract]
pub struct OptionsMarket;

// ─── helpers ─────────────────────────────────────────────────────────────────

fn require_admin(env: &Env) {
    get_admin(env).require_auth();
}

fn check_multisig(env: &Env, multisig_contract: &Address, action_id: u64, class: ActionClass) {
    if !zenith_common::is_executable(env, multisig_contract, action_id, class) {
        panic_with_error!(env, Error::Unauthorized);
    }
}

/// Transfer `amount` of the collateral token from `from` to this contract.
fn pull_token(env: &Env, from: &Address, amount: i128) {
    token::Client::new(env, &get_collateral_token(env)).transfer(
        from,
        &env.current_contract_address(),
        &amount,
    );
}

/// Transfer `amount` of the collateral token from this contract to `to`.
fn push_token(env: &Env, to: &Address, amount: i128) {
    token::Client::new(env, &get_collateral_token(env)).transfer(
        &env.current_contract_address(),
        to,
        &amount,
    );
}

// ─── inner implementations ────────────────────────────────────────────────────

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
    // Expiry must be at least 1 hour out.
    if expiry <= now + 3_600 {
        panic_with_error!(env, Error::ExpiryTooSoon);
    }
    if strike_price <= 0 || premium <= 0 || implied_vol < 0 {
        panic_with_error!(env, Error::InvalidSeriesParams);
    }

    // Active-series cap per underlying (default MAX_SERIES_PER_UNDERLYING = 50).
    let current_count = series_count_for_underlying(env, &underlying);
    if current_count >= MAX_SERIES_PER_UNDERLYING as u64 {
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
    set_series(env, &series);
    set_series_count_for_underlying(env, &underlying, current_count + 1);

    events::series_created(env, series_id, strike_price, expiry, premium);
    series_id
}

fn settle_series_inner(env: &Env, series_id: u64, price: i128) {
    let mut series = require_series(env, series_id);
    if series.state == SeriesState::Settled {
        panic_with_error!(env, Error::AlreadySettled);
    }
    if series.state == SeriesState::Cancelled {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    if env.ledger().timestamp() <= series.expiry {
        panic_with_error!(env, Error::SeriesNotExpired);
    }
    series.state = SeriesState::Settled;
    series.settlement_price = Some(price);
    set_series(env, &series);
    set_underlying_price(env, &series.underlying, price);
    events::settlement_price_set(env, series_id, price);
}

fn exercise_inner(env: &Env, owner: &Address, position_id: u64) -> i128 {
    let mut position = require_position(env, position_id);
    if &position.owner != owner {
        panic_with_error!(env, Error::Unauthorized);
    }
    if position.side != PositionSide::Long {
        panic_with_error!(env, Error::WrongSide);
    }
    if position.is_exercised {
        panic_with_error!(env, Error::AlreadyExercised);
    }
    let series = require_series(env, position.series_id);
    if series.state != SeriesState::Settled {
        if env.ledger().timestamp() <= series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        panic_with_error!(env, Error::PriceNotSet);
    }
    let settlement_price = series
        .settlement_price
        .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));
    let payout = calc_payout(
        position.contracts,
        series.strike_price,
        settlement_price,
        &series.option_type,
    );
    if payout == 0 {
        panic_with_error!(env, Error::NotInTheMoney);
    }
    position.is_exercised = true;
    set_position(env, &position);
    push_token(env, owner, payout);
    events::option_exercised(env, owner.clone(), position_id, settlement_price, payout);
    payout
}

fn reclaim_inner(env: &Env, writer: &Address, position_id: u64) -> i128 {
    let mut position = require_position(env, position_id);
    if &position.owner != writer {
        panic_with_error!(env, Error::Unauthorized);
    }
    if position.side != PositionSide::Short {
        panic_with_error!(env, Error::WrongSide);
    }
    if position.is_settled {
        panic_with_error!(env, Error::AlreadySettled);
    }
    let series = require_series(env, position.series_id);
    if series.state != SeriesState::Settled {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    let settlement_price = series
        .settlement_price
        .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));
    let max_loss = calc_payout(
        position.contracts,
        series.strike_price,
        settlement_price,
        &series.option_type,
    );
    let reclaim = position.collateral_locked - max_loss;
    position.is_settled = true;
    set_position(env, &position);
    if reclaim > 0 {
        push_token(env, writer, reclaim);
    }
    events::collateral_reclaimed(env, writer.clone(), position_id, reclaim);
    reclaim
}

fn set_fee_rate_inner(env: &Env, new_bps: i128) {
    if new_bps < 0 || new_bps > MAX_FEE_RATE_BPS {
        panic_with_error!(env, Error::InvalidFeeRate);
    }
    set_fee_rate_bps(env, new_bps);
    events::fee_rate_updated(env, new_bps);
}

fn pause_inner(env: &Env) {
    env.storage().instance().set(&DataKey::Paused, &true);
    events::paused(env);
}

fn unpause_inner(env: &Env) {
    env.storage().instance().set(&DataKey::Paused, &false);
    events::unpaused(env);
}

fn transfer_admin_inner(env: &Env, new_admin: Address) {
    let old_admin = get_admin(env);
    set_admin(env, &new_admin);
    events::admin_transferred(env, old_admin, new_admin);
}

fn cancel_series_inner(env: &Env, series_id: u64) {
    let mut series = require_active_series(env, series_id);
    series.state = SeriesState::Cancelled;
    set_series(env, &series);
    events::series_cancelled(env, series_id);
}

fn update_premium_inner(env: &Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
    if new_premium <= 0 {
        panic_with_error!(env, Error::InvalidSeriesParams);
    }
    let mut series = require_active_series(env, series_id);
    series.premium = new_premium;
    series.implied_vol = new_implied_vol;
    set_series(env, &series);
    events::premium_updated(env, series_id, new_premium, new_implied_vol);
}

// ─── contract implementation ──────────────────────────────────────────────────

#[contractimpl]
impl OptionsMarket {
    // ── One-time setup ───────────────────────────────────────────────────────

    pub fn initialize(
        env: Env,
        admin: Address,
        oracle: Address,
        collateral_token: Address,
        fee_recipient: Address,
    ) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        ttl::extend_instance(&env);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Oracle, &oracle);
        env.storage()
            .instance()
            .set(&DataKey::CollateralToken, &collateral_token);
        env.storage()
            .instance()
            .set(&DataKey::FeeRecipient, &fee_recipient);
        env.storage()
            .instance()
            .set(&DataKey::FeeRateBps, &DEFAULT_FEE_RATE_BPS);
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    // ── Admin ────────────────────────────────────────────────────────────────

    pub fn transfer_admin(env: Env, new_admin: Address) {
        ttl::extend_instance(&env);
        require_admin(&env);
        transfer_admin_inner(&env, new_admin);
    }

    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        ttl::extend_instance(&env);
        check_multisig(&env, &multisig_contract, action_id, ActionClass::Critical);
        transfer_admin_inner(&env, new_admin);
    }

    pub fn set_fee_rate(env: Env, new_bps: i128) {
        ttl::extend_instance(&env);
        require_admin(&env);
        set_fee_rate_inner(&env, new_bps);
    }

    pub fn set_fee_rate_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_bps: i128,
    ) {
        ttl::extend_instance(&env);
        check_multisig(&env, &multisig_contract, action_id, ActionClass::Standard);
        set_fee_rate_inner(&env, new_bps);
    }

    pub fn pause(env: Env) {
        ttl::extend_instance(&env);
        require_admin(&env);
        pause_inner(&env);
    }

    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        ttl::extend_instance(&env);
        check_multisig(&env, &multisig_contract, action_id, ActionClass::Emergency);
        pause_inner(&env);
    }

    pub fn unpause(env: Env) {
        ttl::extend_instance(&env);
        require_admin(&env);
        unpause_inner(&env);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        ttl::extend_instance(&env);
        check_multisig(&env, &multisig_contract, action_id, ActionClass::Standard);
        unpause_inner(&env);
    }

    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        require_admin(&env);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    pub fn upgrade_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_wasm_hash: BytesN<32>,
    ) {
        ttl::extend_instance(&env);
        check_multisig(&env, &multisig_contract, action_id, ActionClass::Critical);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Admin-only: pin the trusted vault address.
    pub fn set_vault(env: Env, vault: Address) {
        ttl::extend_instance(&env);
        require_admin(&env);
        let old = get_vault(&env);
        set_vault(&env, &vault);
        events::vault_updated(&env, old, vault);
    }

    // ── Series lifecycle ─────────────────────────────────────────────────────

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
        require_admin(&env);
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
        check_multisig(&env, &multisig_contract, action_id, ActionClass::Standard);
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

    pub fn update_premium(env: Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
        ttl::extend_instance(&env);
        require_admin(&env);
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
        ttl::extend_instance(&env);
        check_multisig(&env, &multisig_contract, action_id, ActionClass::Standard);
        update_premium_inner(&env, series_id, new_premium, new_implied_vol);
    }

    pub fn cancel_series(env: Env, series_id: u64) {
        ttl::extend_instance(&env);
        require_admin(&env);
        cancel_series_inner(&env, series_id);
    }

    pub fn cancel_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        series_id: u64,
    ) {
        ttl::extend_instance(&env);
        check_multisig(&env, &multisig_contract, action_id, ActionClass::Standard);
        cancel_series_inner(&env, series_id);
    }

    // ── Oracle settlement ────────────────────────────────────────────────────

    /// Trusted-oracle-address flow: the address set at `initialize` as
    /// `oracle` asserts a settlement price directly.
    pub fn set_settlement_price(env: Env, series_id: u64, price: i128) {
        ttl::extend_instance(&env);
        get_oracle(&env).require_auth();
        settle_series_inner(&env, series_id, price);
    }

    /// Permissionless alternative: cross-calls a live `price_oracle`
    /// deployment to read the current aggregated price.
    pub fn set_settlement_price_from_oracle(env: Env, series_id: u64, oracle_contract: Address) {
        ttl::extend_instance(&env);
        let series = require_series(&env, series_id);
        if env.ledger().timestamp() <= series.expiry {
            panic_with_error!(&env, Error::SeriesNotExpired);
        }
        if series.state == SeriesState::Settled {
            panic_with_error!(&env, Error::AlreadySettled);
        }
        if series.state == SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotActive);
        }
        // Cross-call: get_price(symbol) -> Option<i128>
        let price_opt: Option<i128> = env.invoke_contract(
            &oracle_contract,
            &Symbol::new(&env, "get_price"),
            soroban_sdk::vec![&env, series.underlying.into_val(&env)],
        );
        let price = price_opt.unwrap_or_else(|| panic_with_error!(&env, Error::PriceNotSet));
        settle_series_inner(&env, series_id, price);
    }

    // ── Trading ──────────────────────────────────────────────────────────────

    pub fn buy_option(
        env: Env,
        buyer: Address,
        series_id: u64,
        contracts: i128,
        max_premium: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        buyer.require_auth();
        require_not_paused(&env);

        if contracts <= 0 {
            panic_with_error!(&env, Error::ZeroContracts);
        }
        let series = require_active_series(&env, series_id);
        let total_premium = series.premium * contracts / PRICE_PRECISION;
        if total_premium > max_premium {
            panic_with_error!(&env, Error::InsufficientPremium);
        }
        let bps = fee_rate_bps(&env);
        let fee = calc_fee(total_premium, bps);
        let net_premium = total_premium - fee;

        // Pull total premium from buyer.
        pull_token(&env, &buyer, total_premium);
        // Send fee to fee recipient.
        if fee > 0 {
            push_token(&env, &get_fee_recipient(&env), fee);
        }
        // Net premium goes into the shared pool for writers.
        set_premium_pool(&env, premium_pool(&env) + net_premium);
        // Track escrow liability for this series (in case of cancellation).
        set_series_escrow(&env, series_id, series_escrow(&env, series_id) + net_premium);

        // Update series open interest.
        let mut s = series;
        s.open_interest += contracts;
        set_series(&env, &s);

        // Aggregate stats.
        set_total_premiums_collected(&env, total_premiums_collected(&env) + total_premium);
        set_total_open_interest(&env, total_open_interest(&env) + contracts);

        // Create position.
        let position_id = next_position_id(&env);
        let position = OptionPosition {
            position_id,
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
        set_position(&env, &position);
        add_user_position(&env, &buyer, position_id);

        events::option_bought(&env, buyer, position_id, series_id, contracts, total_premium);
        position_id
    }

    pub fn write_option(
        env: Env,
        writer: Address,
        series_id: u64,
        contracts: i128,
        collateral_amount: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        writer.require_auth();
        require_not_paused(&env);

        if contracts <= 0 {
            panic_with_error!(&env, Error::ZeroContracts);
        }
        let series = require_active_series(&env, series_id);

        // Determine required collateral:
        // - Call: 100% of notional (basis_price * contracts / PRICE_PRECISION)
        //   Falls back to strike when no oracle price is recorded.
        // - Put: 110% of strike (always uses strike, not spot).
        let req_collateral = match series.option_type {
            OptionType::Call => {
                let basis_price = get_underlying_price(&env, &series.underlying)
                    .unwrap_or(series.strike_price);
                required_collateral(&series.option_type, basis_price, contracts)
            }
            OptionType::Put => required_collateral(&series.option_type, series.strike_price, contracts),
        };

        if collateral_amount < req_collateral {
            panic_with_error!(&env, Error::InsufficientCollateral);
        }

        // Net premium the writer will receive (from the shared pool).
        let bps = fee_rate_bps(&env);
        let gross_premium = series.premium * contracts / PRICE_PRECISION;
        let fee = calc_fee(gross_premium, bps);
        let net_premium_to_writer = gross_premium - fee;

        let pool = premium_pool(&env);
        if pool < net_premium_to_writer {
            panic_with_error!(&env, Error::InsufficientPremiumPool);
        }

        // Pull collateral from writer.
        pull_token(&env, &writer, collateral_amount);
        // Pay premium to writer from pool.
        set_premium_pool(&env, pool - net_premium_to_writer);
        if net_premium_to_writer > 0 {
            push_token(&env, &writer, net_premium_to_writer);
        }

        // Track escrow liability for this series' collateral.
        set_series_escrow(
            &env,
            series_id,
            series_escrow(&env, series_id) + collateral_amount,
        );

        // Update series open interest.
        let mut s = series;
        s.open_interest += contracts;
        set_series(&env, &s);
        set_total_open_interest(&env, total_open_interest(&env) + contracts);

        // Create position.
        let position_id = next_position_id(&env);
        let position = OptionPosition {
            position_id,
            series_id,
            owner: writer.clone(),
            side: PositionSide::Short,
            contracts,
            premium_paid: net_premium_to_writer,
            fee_paid: 0,
            collateral_locked: collateral_amount,
            is_exercised: false,
            is_settled: false,
            opened_at: env.ledger().timestamp(),
        };
        set_position(&env, &position);
        add_user_position(&env, &writer, position_id);

        events::option_written(
            &env,
            writer,
            position_id,
            series_id,
            contracts,
            net_premium_to_writer,
            collateral_amount,
        );
        position_id
    }

    pub fn exercise(env: Env, owner: Address, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        exercise_inner(&env, &owner, position_id)
    }

    pub fn exercise_batch(env: Env, owner: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        let len = position_ids.len();
        if len == 0 || len > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut total: i128 = 0;
        for id in position_ids.iter() {
            total += exercise_inner(&env, &owner, id);
        }
        total
    }

    pub fn reclaim_collateral(env: Env, writer: Address, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        writer.require_auth();
        reclaim_inner(&env, &writer, position_id)
    }

    pub fn reclaim_batch(env: Env, writer: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        writer.require_auth();
        let len = position_ids.len();
        if len == 0 || len > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut total: i128 = 0;
        for id in position_ids.iter() {
            total += reclaim_inner(&env, &writer, id);
        }
        total
    }

    // ── Cancellation refunds ─────────────────────────────────────────────────

    pub fn claim_refund(env: Env, owner: Address, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        let mut position = require_position(&env, position_id);
        if &position.owner != &owner {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if position.is_settled {
            panic_with_error!(&env, Error::AlreadySettled);
        }
        let series = require_series(&env, position.series_id);
        if series.state != SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotCancelled);
        }
        let refund = match position.side {
            PositionSide::Long => position.premium_paid - position.fee_paid,
            PositionSide::Short => position.collateral_locked,
        };
        position.is_settled = true;
        set_position(&env, &position);
        // Debit the series' outstanding escrow liability.
        let esc = series_escrow(&env, position.series_id);
        if esc >= refund {
            set_series_escrow(&env, position.series_id, esc - refund);
        }
        push_token(&env, &owner, refund);
        events::refund_claimed(&env, owner, position_id, refund);
        refund
    }

    /// Move a cancelled series' remaining refund liability into a vault.
    pub fn escrow_series_to_vault(env: Env, vault_contract: Address, series_id: u64) {
        ttl::extend_instance(&env);
        let series = require_series(&env, series_id);
        if series.state != SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotCancelled);
        }
        // The vault_contract must be the pinned trusted vault.
        let pinned_vault =
            get_vault(&env).unwrap_or_else(|| panic_with_error!(&env, Error::NotEligibleForForfeiture));
        if vault_contract != pinned_vault {
            panic_with_error!(&env, Error::NotEligibleForForfeiture);
        }
        let amount = series_escrow(&env, series_id);
        if amount == 0 {
            panic_with_error!(&env, Error::NothingToEscrow);
        }
        // Approve vault to pull from this contract, then deposit.
        // We call vault's deposit_legacy(from, id, amount) which pulls from `from`.
        let collateral_token = get_collateral_token(&env);
        token::Client::new(&env, &collateral_token).approve(
            &env.current_contract_address(),
            &vault_contract,
            &amount,
            &(env.ledger().sequence() + 1),
        );
        let _: i128 = env.invoke_contract(
            &vault_contract,
            &Symbol::new(&env, "deposit_legacy"),
            soroban_sdk::vec![
                &env,
                env.current_contract_address().into_val(&env),
                series_id.into_val(&env),
                amount.into_val(&env)
            ],
        );
        // Zero out the series escrow and record which vault holds it.
        set_series_escrow(&env, series_id, 0);
        set_series_vault(&env, series_id, &vault_contract);
        events::series_escrowed_to_vault(&env, series_id, amount);
    }

    pub fn claim_refund_from_vault(
        env: Env,
        vault_contract: Address,
        owner: Address,
        position_id: u64,
    ) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        let mut position = require_position(&env, position_id);
        if &position.owner != &owner {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if position.is_settled {
            panic_with_error!(&env, Error::AlreadySettled);
        }
        let series = require_series(&env, position.series_id);
        if series.state != SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotCancelled);
        }
        // Use the vault recorded at escrow time (not the currently pinned vault).
        let recorded_vault = get_series_vault(&env, position.series_id)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NothingToEscrow));
        if vault_contract != recorded_vault {
            panic_with_error!(&env, Error::Unauthorized);
        }
        let refund = match position.side {
            PositionSide::Long => position.premium_paid - position.fee_paid,
            PositionSide::Short => position.collateral_locked,
        };
        position.is_settled = true;
        set_position(&env, &position);
        // Withdraw from vault.
        env.invoke_contract::<()>(
            &vault_contract,
            &Symbol::new(&env, "withdraw_legacy"),
            soroban_sdk::vec![
                &env,
                position.series_id.into_val(&env),
                owner.clone().into_val(&env),
                refund.into_val(&env)
            ],
        );
        events::refund_claimed(&env, owner, position_id, refund);
        refund
    }

    // ── Views ────────────────────────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        get_admin(&env)
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        storage_is_paused(&env)
    }

    pub fn get_fee_rate(env: Env) -> i128 {
        ttl::extend_instance(&env);
        fee_rate_bps(&env)
    }

    pub fn get_premium_pool(env: Env) -> i128 {
        ttl::extend_instance(&env);
        premium_pool(&env)
    }

    /// Returns `(total_premiums_collected, total_open_interest, series_count)`.
    pub fn get_stats(env: Env) -> (i128, i128, u64) {
        ttl::extend_instance(&env);
        (
            total_premiums_collected(&env),
            total_open_interest(&env),
            series_counter(&env),
        )
    }

    pub fn get_series(env: Env, series_id: u64) -> Option<OptionSeries> {
        ttl::extend_instance(&env);
        get_series(&env, series_id)
    }

    pub fn get_position(env: Env, position_id: u64) -> Option<OptionPosition> {
        ttl::extend_instance(&env);
        get_position(&env, position_id)
    }

    pub fn get_user_positions(env: Env, user: Address) -> Vec<u64> {
        ttl::extend_instance(&env);
        get_user_positions(&env, &user)
    }

    pub fn get_series_count_for_underlying(env: Env, underlying: Symbol) -> u64 {
        ttl::extend_instance(&env);
        series_count_for_underlying(&env, &underlying)
    }

    pub fn get_series_escrow(env: Env, series_id: u64) -> i128 {
        ttl::extend_instance(&env);
        series_escrow(&env, series_id)
    }

    pub fn get_vault(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        get_vault(&env)
    }

    pub fn get_series_vault(env: Env, series_id: u64) -> Option<Address> {
        ttl::extend_instance(&env);
        get_series_vault(&env, series_id)
    }

    pub fn get_underlying_price(env: Env, underlying: Symbol) -> Option<i128> {
        ttl::extend_instance(&env);
        get_underlying_price(&env, &underlying)
    }
}
