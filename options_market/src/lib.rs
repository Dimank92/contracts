#![no_std]

//! Zenith Protocol — Decentralized Options Market on Stellar Soroban
//!
//! Supports European-style put and call options on XLM, BTC, ETH, and SOL.
//! Premium is set by the admin (computed off-chain via Black-Scholes).
//! Writers lock collateral; buyers pay premium. Settlement at expiry via
//! the oracle: either the trusted oracle account signs
//! `set_settlement_price`, or anyone calls `set_settlement_price_from_oracle`
//! to read a live price_oracle deployment.
//!
//! **Vault-backed series.** A series created while a vault is pinned
//! (`set_vault`) escrows every writer's collateral in that vault under tag
//! `series_id`, and pays exercises, reclaims and collateral refunds out of
//! that tag only — so one series' payouts can never draw on another
//! series' collateral. Series created with no vault pinned keep the
//! original behaviour (collateral held in this contract's own balance).
//!
//! Every admin function is a thin entrypoint over an `x_inner(env, Auth,
//! ...)` that starts with `zenith_common::require_admin_or_multisig`; the
//! `_via_multisig` twins only differ in the `Auth` they pass.

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contractimpl, panic_with_error, vec, Address, BytesN, Env, IntoVal, Symbol, Vec,
};
use zenith_common::{self as common, ActionClass, Auth};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_adversaries;

mod error;
mod events;
pub mod math;
mod migrations;
mod params_client;
mod price_oracle_client;
#[allow(dead_code)]
pub mod rounding_policy;
mod storage;
mod ttl;
mod types;
mod vault_client;

use error::Error;
use math::{
    calc_fee, calc_payout, DEFAULT_FEE_RATE_BPS, MAX_BATCH_SIZE, MAX_FEE_RATE_BPS,
    MAX_SERIES_PER_UNDERLYING, MIN_COLLATERAL_RATIO, PRICE_PRECISION, RATE_PRECISION,
};
use storage::{
    add_user_position, bump_instance_i128, collateral_token, collateral_vault, fee_rate_bps,
    get_position, get_series, instance_get, instance_i128, next_position_id, put_position,
    put_series, require_active_series, require_not_paused, settlement_window,
};
pub use types::{DataKey, OptionPosition, OptionSeries, OptionType, PositionSide, SeriesState};

fn gate(env: &Env, auth: &Auth, class: ActionClass) {
    common::require_admin_or_multisig(env, &DataKey::Admin, auth, class, Error::Unauthorized);
}

/// Unwraps checked arithmetic, failing with a typed error instead of a
/// bare panic.
fn ck(env: &Env, value: Option<i128>) -> i128 {
    value.unwrap_or_else(|| panic_with_error!(env, Error::InvalidSeriesParams))
}

/// `contracts × price / PRICE_PRECISION`, checked.
fn notional(env: &Env, contracts: i128, price: i128) -> i128 {
    ck(
        env,
        contracts
            .checked_mul(price)
            .and_then(|v| v.checked_div(PRICE_PRECISION)),
    )
}

/// Deposits `amount` of this contract's own collateral token balance into
/// `vault` under `tag`. The vault pulls the tokens with
/// `token.transfer(this, vault, amount)`, one call below this contract, so
/// that transfer is authorized explicitly here.
fn vault_deposit(env: &Env, vault: &Address, tag: u64, amount: i128) {
    let this = env.current_contract_address();
    let token = collateral_token(env);
    env.authorize_as_current_contract(vec![
        env,
        InvokerContractAuthEntry::Contract(SubContractInvocation {
            context: ContractContext {
                contract: token.address.clone(),
                fn_name: Symbol::new(env, "transfer"),
                args: (this.clone(), vault.clone(), amount).into_val(env),
            },
            sub_invocations: vec![env],
        }),
    ]);
    vault_client::Client::new(env, vault).deposit(&this, &tag, &amount);
}

/// Pays `amount` of a series' collateral-backed funds to `to`: out of the
/// series' own vault tag for a vault-backed series, otherwise out of this
/// contract's balance.
fn pay_from_collateral(env: &Env, series_id: u64, to: &Address, amount: i128) {
    if amount <= 0 {
        return;
    }
    match collateral_vault(env, series_id) {
        Some(vault) => vault_client::Client::new(env, &vault).withdraw(&series_id, to, &amount),
        None => collateral_token(env).transfer(&env.current_contract_address(), to, &amount),
    }
}

/// Moves `amount` of a series' outstanding refund liability up or down.
fn adjust_series_escrow(env: &Env, series_id: u64, delta: i128) {
    let key = DataKey::SeriesEscrow(series_id);
    let outstanding: i128 = ttl::get_persistent(env, &key).unwrap_or(0);
    ttl::set_persistent(env, &key, &ck(env, outstanding.checked_add(delta)));
}

#[contract]
pub struct OptionsMarket;

#[contractimpl]
impl OptionsMarket {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs. Anyone may pay the rent to keep long-lived entries alive.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    // ── Initialization ────────────────────────────────────────────────────────

    pub fn initialize(
        env: Env,
        admin: Address,
        oracle: Address,
        collateral_token: Address,
        fee_recipient: Address,
    ) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        let s = env.storage().instance();
        s.set(&DataKey::Admin, &admin);
        s.set(&DataKey::Oracle, &oracle);
        s.set(&DataKey::CollateralToken, &collateral_token);
        s.set(&DataKey::FeeRecipient, &fee_recipient);
        s.set(&DataKey::SeriesCounter, &0u64);
        s.set(&DataKey::PositionCounter, &0u64);
        s.set(&DataKey::TotalPremiumsCollected, &0i128);
        s.set(&DataKey::TotalOpenInterest, &0i128);
        s.set(&DataKey::FeeRateBps, &DEFAULT_FEE_RATE_BPS);
        s.set(&DataKey::PremiumPool, &0i128);
        common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    // ── Admin: configuration ──────────────────────────────────────────────────

    /// Protocol fee rate in basis points, capped at MAX_FEE_RATE_BPS so a
    /// compromised or careless admin can't confiscate every trade.
    pub fn set_fee_rate(env: Env, new_bps: u32) {
        Self::set_fee_rate_inner(&env, Auth::Admin, new_bps);
    }

    pub fn set_fee_rate_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        new_bps: u32,
    ) {
        Self::set_fee_rate_inner(&env, Auth::Multisig(multisig_contract, action_id), new_bps);
    }

    /// Points fee rate and settlement window at a params registry (see
    /// ../params). While set, its values override `set_fee_rate` on every
    /// registry version change.
    pub fn set_params_registry(env: Env, registry: Address) {
        Self::set_params_registry_inner(&env, Auth::Admin, registry);
    }

    pub fn set_params_registry_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        registry: Address,
    ) {
        Self::set_params_registry_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            registry,
        );
    }

    /// Re-points the settlement oracle: the account that may sign
    /// `set_settlement_price`, and the only price_oracle contract
    /// `set_settlement_price_from_oracle` will read.
    pub fn set_oracle(env: Env, oracle: Address) {
        Self::set_oracle_inner(&env, Auth::Admin, oracle);
    }

    pub fn set_oracle_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        oracle: Address,
    ) {
        Self::set_oracle_inner(&env, Auth::Multisig(multisig_contract, action_id), oracle);
    }

    /// Pins the vault new series escrow their writers' collateral in, and
    /// the only vault `escrow_series_to_vault` accepts for series that
    /// don't have one yet. Series keep the vault they were created with.
    pub fn set_vault(env: Env, vault: Address) {
        Self::set_vault_inner(&env, Auth::Admin, vault);
    }

    pub fn set_vault_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        vault: Address,
    ) {
        Self::set_vault_inner(&env, Auth::Multisig(multisig_contract, action_id), vault);
    }

    // ── Admin: transfer (two-step) ────────────────────────────────────────────

    /// Step one of an admin transfer: records `new_admin` as pending.
    /// Control only moves once `new_admin` calls `accept_admin`, so a
    /// wrong or unusable address can't strand the contract. Proposing the
    /// current admin cancels a pending transfer.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        Self::transfer_admin_inner(&env, Auth::Admin, new_admin);
    }

    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        new_admin: Address,
    ) {
        Self::transfer_admin_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            new_admin,
        );
    }

    /// Step two: the pending admin takes over.
    pub fn accept_admin(env: Env) {
        ttl::extend_instance(&env);
        let (old, new) = common::accept_admin(&env, &DataKey::Admin, Error::NoPendingAdmin);
        events::admin_transferred(&env, old, new);
    }

    pub fn get_pending_admin(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        common::pending_admin(&env)
    }

    /// Accepts a pending admin transfer of `vault` to this contract — the
    /// vault's `accept_admin` needs this contract's own authorization,
    /// which only this contract can give. Admin only.
    pub fn accept_vault_admin(env: Env, vault: Address) {
        ttl::extend_instance(&env);
        gate(&env, &Auth::Admin, ActionClass::Critical);
        vault_client::Client::new(&env, &vault).accept_admin();
    }

    // ── Admin: pause ──────────────────────────────────────────────────────────

    /// Emergency stop: blocks new series and new trades (create_series,
    /// update_premium, buy_option, write_option). Does NOT block exercise,
    /// settlement, reclaims or refunds — a pause should let existing
    /// positions wind down, not trap funds.
    pub fn pause(env: Env) {
        Self::set_paused_inner(&env, Auth::Admin, true);
    }

    pub fn unpause(env: Env) {
        Self::set_paused_inner(&env, Auth::Admin, false);
    }

    /// `Emergency` class: the short path that stays available when the
    /// admin itself is a timelock.
    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        Self::set_paused_inner(&env, Auth::Multisig(multisig_contract, action_id), true);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        Self::set_paused_inner(&env, Auth::Multisig(multisig_contract, action_id), false);
    }

    // ── Admin: series management ──────────────────────────────────────────────

    /// Lists a new option series (strike + expiry + type). Premium is
    /// computed off-chain via Black-Scholes and passed in.
    pub fn create_series(
        env: Env,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
        premium: i128,
        implied_vol: i128,
    ) -> u64 {
        Self::create_series_inner(
            &env,
            Auth::Admin,
            underlying,
            option_type,
            strike_price,
            expiry,
            premium,
            implied_vol,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
        premium: i128,
        implied_vol: i128,
    ) -> u64 {
        Self::create_series_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            underlying,
            option_type,
            strike_price,
            expiry,
            premium,
            implied_vol,
        )
    }

    pub fn update_premium(env: Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
        Self::update_premium_inner(&env, Auth::Admin, series_id, new_premium, new_implied_vol);
    }

    pub fn update_premium_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        series_id: u64,
        new_premium: i128,
        new_implied_vol: i128,
    ) {
        Self::update_premium_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            series_id,
            new_premium,
            new_implied_vol,
        );
    }

    /// Cancels an active series; every position in it becomes refundable
    /// (`claim_refund` / `claim_refund_from_vault`).
    pub fn cancel_series(env: Env, series_id: u64) {
        Self::cancel_series_inner(&env, Auth::Admin, series_id);
    }

    pub fn cancel_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        series_id: u64,
    ) {
        Self::cancel_series_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            series_id,
        );
    }

    // ── Refunds for cancelled series ──────────────────────────────────────────

    /// Refunds a position in a cancelled series: a long gets its premium
    /// back net of the fee it paid; a short gets its full collateral
    /// (from the series' vault tag when the series is vault-backed).
    pub fn claim_refund(env: Env, owner: Address, position_id: u64) {
        ttl::extend_instance(&env);
        owner.require_auth();
        let (mut position, refund) = Self::refundable(&env, &owner, position_id);

        match position.side {
            PositionSide::Long => {
                if refund > 0 {
                    collateral_token(&env).transfer(
                        &env.current_contract_address(),
                        &owner,
                        &refund,
                    );
                }
                adjust_series_escrow(&env, position.series_id, -refund);
            }
            PositionSide::Short => {
                if collateral_vault(&env, position.series_id).is_none() {
                    adjust_series_escrow(&env, position.series_id, -refund);
                }
                pay_from_collateral(&env, position.series_id, &owner, refund);
            }
        }

        position.is_settled = true;
        put_position(&env, &position);
        events::refund_claimed(&env, owner, position_id, refund);
    }

    /// Moves a cancelled series' outstanding refund liability (whatever is
    /// still held in this contract's own balance for it) into a vault
    /// under tag `series_id`, so refunds no longer depend on this
    /// contract's undifferentiated balance. Permissionless. `vault_contract`
    /// must be the series' own vault if it has one, otherwise the pinned
    /// vault.
    pub fn escrow_series_to_vault(env: Env, vault_contract: Address, series_id: u64) {
        ttl::extend_instance(&env);
        migrations::require_current(&env);
        let expected: Option<Address> = ttl::get_persistent(&env, &DataKey::SeriesVault(series_id))
            .or_else(|| instance_get(&env, &DataKey::Vault));
        if expected.as_ref() != Some(&vault_contract) {
            panic_with_error!(&env, Error::UnauthorizedVault);
        }
        let series = get_series(&env, series_id);
        if series.state != SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotCancelled);
        }

        let key = DataKey::SeriesEscrow(series_id);
        let amount: i128 = ttl::get_persistent(&env, &key).unwrap_or(0);
        if amount <= 0 {
            panic_with_error!(&env, Error::NothingToEscrow);
        }
        ttl::set_persistent(&env, &key, &0i128);
        ttl::set_persistent(&env, &DataKey::SeriesVault(series_id), &vault_contract);

        let vault = vault_client::Client::new(&env, &vault_contract);
        let before = vault.balance_of(&series_id);
        vault_deposit(&env, &vault_contract, series_id, amount);
        // Post-condition: the series' tag grew by exactly what was moved.
        if vault.balance_of(&series_id) != ck(&env, before.checked_add(amount)) {
            panic_with_error!(&env, Error::UnauthorizedVault);
        }

        events::series_escrowed_to_vault(&env, series_id, amount);
    }

    /// `claim_refund`, paid out of the series' vault tag. `vault_contract`
    /// must be the vault recorded for the series.
    pub fn claim_refund_from_vault(
        env: Env,
        vault_contract: Address,
        owner: Address,
        position_id: u64,
    ) {
        ttl::extend_instance(&env);
        owner.require_auth();
        let (mut position, refund) = Self::refundable(&env, &owner, position_id);
        let series_vault: Option<Address> =
            ttl::get_persistent(&env, &DataKey::SeriesVault(position.series_id));
        if series_vault.as_ref() != Some(&vault_contract) {
            panic_with_error!(&env, Error::UnauthorizedVault);
        }

        if refund > 0 {
            vault_client::Client::new(&env, &vault_contract).withdraw(
                &position.series_id,
                &owner,
                &refund,
            );
        }
        let collateral_in_vault = collateral_vault(&env, position.series_id).is_some();
        if position.side == PositionSide::Long || !collateral_in_vault {
            adjust_series_escrow(&env, position.series_id, -refund);
        }

        position.is_settled = true;
        put_position(&env, &position);
        events::refund_claimed(&env, owner, position_id, refund);
    }

    /// Refund-eligible principal of a cancelled series still held in this
    /// contract's own balance (what `escrow_series_to_vault` would move).
    pub fn get_series_escrow(env: Env, series_id: u64) -> i128 {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::SeriesEscrow(series_id)).unwrap_or(0)
    }

    // ── Trading ───────────────────────────────────────────────────────────────

    /// Buys `contracts` of a series, paying at most `max_premium`.
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
        migrations::require_current(&env);
        if contracts <= 0 {
            panic_with_error!(&env, Error::ZeroContracts);
        }

        let mut series = require_active_series(&env, series_id);
        let total_premium = notional(&env, contracts, series.premium);
        if total_premium > max_premium {
            panic_with_error!(&env, Error::InsufficientPremium);
        }
        let fee = calc_fee(total_premium, fee_rate_bps(&env));
        let premium_after_fee = total_premium - fee;

        let usdc = collateral_token(&env);
        let this = env.current_contract_address();
        usdc.transfer(&buyer, &this, &total_premium);
        if fee > 0 {
            let fee_recipient: Address = instance_get(&env, &DataKey::FeeRecipient)
                .unwrap_or_else(|| panic_with_error!(&env, Error::Unauthorized));
            usdc.transfer(&this, &fee_recipient, &fee);
        }

        let pos_id = next_position_id(&env);
        put_position(
            &env,
            &OptionPosition {
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
            },
        );
        add_user_position(&env, &buyer, pos_id);

        series.open_interest = ck(&env, series.open_interest.checked_add(contracts));
        put_series(&env, &series);

        bump_instance_i128(&env, &DataKey::TotalPremiumsCollected, premium_after_fee);
        bump_instance_i128(&env, &DataKey::TotalOpenInterest, contracts);
        // Funds the pool writers get paid out of — see write_option.
        bump_instance_i128(&env, &DataKey::PremiumPool, premium_after_fee);
        // This position's own refund-eligible principal, should the series
        // be cancelled.
        adjust_series_escrow(&env, series_id, premium_after_fee);

        events::option_bought(&env, buyer, pos_id, series_id, contracts, total_premium);
        pos_id
    }

    /// Writes `contracts` of a series: locks collateral (covered call:
    /// notional at the last known underlying price, or the strike; cash-
    /// secured put: 110% of strike notional) and receives the premium,
    /// net of fee, from the pool buyers' premiums fund. Rejected if the
    /// pool can't cover it yet or the premium is below `min_premium`.
    pub fn write_option(
        env: Env,
        writer: Address,
        series_id: u64,
        contracts: i128,
        collateral_amount: i128,
        min_premium: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        writer.require_auth();
        require_not_paused(&env);
        migrations::require_current(&env);
        if contracts <= 0 {
            panic_with_error!(&env, Error::ZeroContracts);
        }

        let mut series = require_active_series(&env, series_id);
        let required_collateral = match series.option_type {
            OptionType::Call => {
                let underlying_price: i128 =
                    ttl::get_persistent(&env, &DataKey::UnderlyingPrice(series.underlying.clone()))
                        .unwrap_or(series.strike_price);
                notional(&env, contracts, underlying_price)
            }
            OptionType::Put => {
                let nominal = notional(&env, contracts, series.strike_price);
                ck(
                    &env,
                    nominal
                        .checked_mul(MIN_COLLATERAL_RATIO)
                        .and_then(|v| v.checked_div(RATE_PRECISION)),
                )
            }
        };
        if collateral_amount < required_collateral {
            panic_with_error!(&env, Error::InsufficientCollateral);
        }

        // Writer premium comes out of the pool buyers' premiums fund —
        // never out of collateral (including this writer's own).
        let total_premium = notional(&env, contracts, series.premium);
        let fee = calc_fee(total_premium, fee_rate_bps(&env));
        let writer_premium = total_premium - fee;
        if writer_premium < min_premium {
            panic_with_error!(&env, Error::PremiumBelowMinimum);
        }
        let pool = instance_i128(&env, &DataKey::PremiumPool);
        if pool < writer_premium {
            panic_with_error!(&env, Error::InsufficientPremiumPool);
        }

        let usdc = collateral_token(&env);
        let this = env.current_contract_address();
        usdc.transfer(&writer, &this, &required_collateral);
        match collateral_vault(&env, series_id) {
            Some(vault) => vault_deposit(&env, &vault, series_id, required_collateral),
            None => adjust_series_escrow(&env, series_id, required_collateral),
        }

        env.storage()
            .instance()
            .set(&DataKey::PremiumPool, &(pool - writer_premium));
        usdc.transfer(&this, &writer, &writer_premium);

        let pos_id = next_position_id(&env);
        put_position(
            &env,
            &OptionPosition {
                position_id: pos_id,
                series_id,
                owner: writer.clone(),
                side: PositionSide::Short,
                contracts,
                premium_paid: writer_premium,
                fee_paid: 0,
                collateral_locked: required_collateral,
                is_exercised: false,
                is_settled: false,
                opened_at: env.ledger().timestamp(),
            },
        );
        add_user_position(&env, &writer, pos_id);

        series.open_interest = ck(&env, series.open_interest.checked_add(contracts));
        put_series(&env, &series);
        bump_instance_i128(&env, &DataKey::TotalOpenInterest, contracts);

        events::option_written(
            &env,
            writer,
            pos_id,
            series_id,
            contracts,
            writer_premium,
            required_collateral,
        );
        pos_id
    }

    // ── Exercise and settlement ───────────────────────────────────────────────

    /// Exercises an in-the-money long after expiry, within the settlement
    /// window. Settlement price must be set first.
    pub fn exercise(env: Env, owner: Address, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        Self::exercise_one(&env, &owner, position_id)
    }

    /// All-or-nothing: any failing position reverts the whole batch.
    pub fn exercise_batch(env: Env, owner: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        if position_ids.is_empty() || position_ids.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut total: i128 = 0;
        for position_id in position_ids.iter() {
            let payout = Self::exercise_one(&env, &owner, position_id);
            total = ck(&env, total.checked_add(payout));
        }
        total
    }

    /// The trusted oracle account sets a series' settlement price after
    /// expiry. One-shot: a settled series can't be re-priced.
    pub fn set_settlement_price(env: Env, series_id: u64, price: i128) {
        ttl::extend_instance(&env);
        let oracle: Address = instance_get(&env, &DataKey::Oracle)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Unauthorized));
        oracle.require_auth();
        Self::settle(&env, series_id, price);
    }

    /// Permissionless settlement: reads `oracle_contract.get_price` for the
    /// series' underlying. `oracle_contract` must be this market's
    /// configured oracle (`set_oracle`) — a caller can't settle against an
    /// oracle of their choosing. Fails with `PriceNotSet` when the oracle
    /// has no fresh, quorum-backed price.
    pub fn set_settlement_price_from_oracle(env: Env, series_id: u64, oracle_contract: Address) {
        ttl::extend_instance(&env);
        let oracle: Option<Address> = instance_get(&env, &DataKey::Oracle);
        if oracle.as_ref() != Some(&oracle_contract) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        let series = get_series(&env, series_id);
        if env.ledger().timestamp() < series.expiry {
            panic_with_error!(&env, Error::SeriesNotExpired);
        }
        let price = price_oracle_client::Client::new(&env, &oracle_contract)
            .get_price(&series.underlying)
            .unwrap_or_else(|| panic_with_error!(&env, Error::PriceNotSet));
        Self::settle(&env, series_id, price);
    }

    /// Returns a short's collateral minus what its contracts owe at the
    /// settlement price.
    pub fn reclaim_collateral(env: Env, writer: Address, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        writer.require_auth();
        Self::reclaim_one(&env, &writer, position_id)
    }

    /// All-or-nothing: any failing position reverts the whole batch.
    pub fn reclaim_batch(env: Env, writer: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        writer.require_auth();
        if position_ids.is_empty() || position_ids.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut total: i128 = 0;
        for position_id in position_ids.iter() {
            let reclaim = Self::reclaim_one(&env, &writer, position_id);
            total = ck(&env, total.checked_add(reclaim));
        }
        total
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        common::get_admin(&env, &DataKey::Admin)
    }

    pub fn get_oracle(env: Env) -> Address {
        ttl::extend_instance(&env);
        instance_get(&env, &DataKey::Oracle)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Unauthorized))
    }

    pub fn get_vault(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        instance_get(&env, &DataKey::Vault)
    }

    /// The vault holding a series' escrow, if any.
    pub fn get_series_vault(env: Env, series_id: u64) -> Option<Address> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::SeriesVault(series_id))
    }

    /// True when the series' writer collateral lives in its vault tag.
    pub fn is_vault_backed(env: Env, series_id: u64) -> bool {
        ttl::extend_instance(&env);
        collateral_vault(&env, series_id).is_some()
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        common::is_paused(&env, &DataKey::Paused)
    }

    pub fn get_fee_rate(env: Env) -> i128 {
        ttl::extend_instance(&env);
        fee_rate_bps(&env)
    }

    pub fn get_settlement_window(env: Env) -> u64 {
        ttl::extend_instance(&env);
        settlement_window(&env)
    }

    pub fn get_premium_pool(env: Env) -> i128 {
        ttl::extend_instance(&env);
        instance_i128(&env, &DataKey::PremiumPool)
    }

    pub fn get_series_count_for_underlying(env: Env, underlying: Symbol) -> u32 {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::SeriesCountForUnderlying(underlying)).unwrap_or(0)
    }

    pub fn get_series(env: Env, series_id: u64) -> Option<OptionSeries> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::Series(series_id))
    }

    pub fn get_position(env: Env, position_id: u64) -> Option<OptionPosition> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::Position(position_id))
    }

    pub fn get_user_positions(env: Env, user: Address) -> Vec<u64> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::UserPositions(user)).unwrap_or_else(|| Vec::new(&env))
    }

    pub fn get_underlying_price(env: Env, underlying: Symbol) -> Option<i128> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::UnderlyingPrice(underlying))
    }

    /// `(total premiums collected net of fees, total open interest, series count)`.
    pub fn get_stats(env: Env) -> (i128, i128, u64) {
        ttl::extend_instance(&env);
        (
            instance_i128(&env, &DataKey::TotalPremiumsCollected),
            instance_i128(&env, &DataKey::TotalOpenInterest),
            instance_get(&env, &DataKey::SeriesCounter).unwrap_or(0),
        )
    }
}

/// Admin bodies shared by each entrypoint and its `_via_multisig` twin,
/// plus the per-position settlement steps. Auth is resolved first, before
/// any other state is read.
impl OptionsMarket {
    fn set_fee_rate_inner(env: &Env, auth: Auth, new_bps: u32) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Standard);
        let new_bps = new_bps as i128;
        if new_bps > MAX_FEE_RATE_BPS {
            panic_with_error!(env, Error::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::FeeRateBps, &new_bps);
        events::fee_rate_updated(env, new_bps);
    }

    fn set_params_registry_inner(env: &Env, auth: Auth, registry: Address) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Standard);
        env.storage()
            .instance()
            .set(&DataKey::ParamsRegistry, &registry);
        env.storage().instance().remove(&DataKey::ParamsVersion);
        events::params_registry_updated(env, registry);
    }

    fn set_oracle_inner(env: &Env, auth: Auth, oracle: Address) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Critical);
        let old: Address = instance_get(env, &DataKey::Oracle).unwrap_or_else(|| oracle.clone());
        env.storage().instance().set(&DataKey::Oracle, &oracle);
        events::oracle_updated(env, old, oracle);
    }

    fn set_vault_inner(env: &Env, auth: Auth, vault: Address) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Critical);
        let old: Option<Address> = instance_get(env, &DataKey::Vault);
        env.storage().instance().set(&DataKey::Vault, &vault);
        events::vault_updated(env, old, vault);
    }

    fn transfer_admin_inner(env: &Env, auth: Auth, new_admin: Address) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Critical);
        common::propose_admin(env, &DataKey::Admin, &new_admin);
        events::admin_transfer_proposed(env, common::get_admin(env, &DataKey::Admin), new_admin);
    }

    fn set_paused_inner(env: &Env, auth: Auth, paused: bool) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Emergency);
        common::set_paused(env, &DataKey::Paused, paused);
        if paused {
            events::paused(env);
        } else {
            events::unpaused(env);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_series_inner(
        env: &Env,
        auth: Auth,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
        premium: i128,
        implied_vol: i128,
    ) -> u64 {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Standard);
        require_not_paused(env);
        migrations::require_current(env);

        let now = env.ledger().timestamp();
        if expiry <= now.saturating_add(3600) {
            panic_with_error!(env, Error::ExpiryTooSoon);
        }
        if strike_price <= 0 || premium <= 0 || implied_vol < 0 {
            panic_with_error!(env, Error::InvalidSeriesParams);
        }

        let count_key = DataKey::SeriesCountForUnderlying(underlying.clone());
        let count: u32 = ttl::get_persistent(env, &count_key).unwrap_or(0);
        if count >= MAX_SERIES_PER_UNDERLYING {
            panic_with_error!(env, Error::TooManySeriesForUnderlying);
        }
        ttl::set_persistent(env, &count_key, &(count + 1));

        let counter: u64 = instance_get(env, &DataKey::SeriesCounter).unwrap_or(0);
        let series_id = counter
            .checked_add(1)
            .unwrap_or_else(|| panic_with_error!(env, Error::InvalidSeriesParams));
        env.storage()
            .instance()
            .set(&DataKey::SeriesCounter, &series_id);

        put_series(
            env,
            &OptionSeries {
                series_id,
                underlying,
                option_type,
                strike_price,
                expiry,
                premium,
                implied_vol,
                open_interest: 0,
                state: SeriesState::Active,
                settlement_price: None,
                created_at: now,
            },
        );

        // A series listed while a vault is pinned escrows its writers'
        // collateral there for its whole life, even if the pin moves.
        if let Some(vault) = instance_get::<Address>(env, &DataKey::Vault) {
            ttl::set_persistent(env, &DataKey::SeriesVault(series_id), &vault);
            ttl::set_persistent(env, &DataKey::CollateralInVault(series_id), &true);
        }

        events::series_created(env, series_id, strike_price, expiry, premium);
        series_id
    }

    fn update_premium_inner(
        env: &Env,
        auth: Auth,
        series_id: u64,
        new_premium: i128,
        new_implied_vol: i128,
    ) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Standard);
        require_not_paused(env);

        let mut series = get_series(env, series_id);
        if series.state != SeriesState::Active {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        // A zero (or negative) premium would let buyers open positions for
        // free and writers lock collateral for nothing (#47).
        if new_premium <= 0 {
            panic_with_error!(env, Error::InvalidSeriesParams);
        }
        series.premium = new_premium;
        series.implied_vol = new_implied_vol;
        put_series(env, &series);
        events::premium_updated(env, series_id, new_premium, new_implied_vol);
    }

    fn cancel_series_inner(env: &Env, auth: Auth, series_id: u64) {
        ttl::extend_instance(env);
        gate(env, &auth, ActionClass::Standard);

        let mut series = get_series(env, series_id);
        if series.state != SeriesState::Active {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        series.state = SeriesState::Cancelled;
        put_series(env, &series);
        events::series_cancelled(env, series_id);
    }

    /// Writes a series' one and only settlement price.
    fn settle(env: &Env, series_id: u64, price: i128) {
        let mut series = get_series(env, series_id);
        if env.ledger().timestamp() < series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        match series.state {
            SeriesState::Settled => panic_with_error!(env, Error::AlreadySettled),
            SeriesState::Cancelled => panic_with_error!(env, Error::SeriesNotActive),
            SeriesState::Active | SeriesState::Expired => {}
        }
        if price <= 0 {
            panic_with_error!(env, Error::PriceNotSet);
        }
        series.settlement_price = Some(price);
        series.state = SeriesState::Settled;
        put_series(env, &series);
        ttl::set_persistent(
            env,
            &DataKey::UnderlyingPrice(series.underlying.clone()),
            &price,
        );
        events::settlement_price_set(env, series_id, price);
    }

    /// Checks `owner` may refund `position_id` (cancelled series, still
    /// open) and returns it with the refund amount.
    fn refundable(env: &Env, owner: &Address, position_id: u64) -> (OptionPosition, i128) {
        let position = get_position(env, position_id);
        if position.owner != *owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }
        if get_series(env, position.series_id).state != SeriesState::Cancelled {
            panic_with_error!(env, Error::SeriesNotCancelled);
        }
        // Longs get premium back net of the fee actually deducted at buy
        // time (not a recomputation at today's rate): the fee itself was
        // already sent to fee_recipient.
        let refund = match position.side {
            PositionSide::Long => ck(env, position.premium_paid.checked_sub(position.fee_paid)),
            PositionSide::Short => position.collateral_locked,
        };
        (position, refund)
    }

    fn exercise_one(env: &Env, owner: &Address, position_id: u64) -> i128 {
        let mut position = get_position(env, position_id);
        if position.owner != *owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.side != PositionSide::Long {
            panic_with_error!(env, Error::WrongSide);
        }
        if position.is_exercised {
            panic_with_error!(env, Error::AlreadyExercised);
        }

        let series = get_series(env, position.series_id);
        let now = env.ledger().timestamp();
        // European: exercise only after expiry, within the settlement window.
        if now < series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        if now > series.expiry.saturating_add(settlement_window(env)) {
            panic_with_error!(env, Error::ExerciseWindowClosed);
        }
        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));

        let payout = calc_payout(
            &series.option_type,
            series.strike_price,
            settlement_price,
            position.contracts,
        );
        if payout <= 0 {
            panic_with_error!(env, Error::NotInTheMoney);
        }

        position.is_exercised = true;
        put_position(env, &position);
        pay_from_collateral(env, position.series_id, owner, payout);

        events::option_exercised(env, owner.clone(), position_id, settlement_price, payout);
        payout
    }

    fn reclaim_one(env: &Env, writer: &Address, position_id: u64) -> i128 {
        let mut position = get_position(env, position_id);
        if position.owner != *writer {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.side != PositionSide::Short {
            panic_with_error!(env, Error::WrongSide);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }

        let series = get_series(env, position.series_id);
        if series.state != SeriesState::Settled {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));

        // What this short's contracts owe the longs at settlement.
        let max_loss = calc_payout(
            &series.option_type,
            series.strike_price,
            settlement_price,
            position.contracts,
        );
        let reclaim = position.collateral_locked.saturating_sub(max_loss).max(0);

        position.is_settled = true;
        put_position(env, &position);
        pay_from_collateral(env, position.series_id, writer, reclaim);

        events::collateral_reclaimed(env, writer.clone(), position_id, reclaim);
        reclaim
    }
}
