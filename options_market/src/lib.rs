#![no_std]

//! Zenith Protocol — Decentralized Options Market on Stellar Soroban
//!
//! European-style put and call options on XLM, BTC, ETH, and SOL.
//! Premium is set by the admin (computed off-chain via Black-Scholes).
//! Writers lock collateral; buyers pay premium. Settlement at expiry via oracle.
//!
//! ## Admin surface (#192)
//!
//! Every admin entrypoint has a `_via_multisig` twin, and both go through
//! one helper, [`authorize`], so the admin can be a bare address, a
//! multisig or a timelock. The levers:
//!
//! - **Premium**: `create_series` / `update_premium` set the per-contract
//!   premium, bounded by `math::max_premium` (a put is never worth more
//!   than its strike, a call never more than the underlying). Premium can
//!   be changed while a series is open, including after purchases: the
//!   change only prices *future* purchases — every position records the
//!   premium and fee it actually paid, and refunds use those figures.
//! - **Halt**: `halt_series` / `unhalt_series` stop and resume new
//!   purchases and writes on one series via an explicit storage flag.
//! - **Pause**: `pause` / `unpause` stop new series, premium changes,
//!   purchases and writes contract-wide.
//!
//! Neither halt nor pause blocks settlement, exercise, reclaim or refund
//! claims: they stop new risk, they never trap funds.
//!
//! ## Settlement (#191)
//!
//! After expiry a series settles once, at one price — through the trusted
//! `oracle` (`set_settlement_price`), a caller-named `price_oracle`
//! (`set_settlement_price_from_oracle`) or, permissionlessly, the pinned
//! `price_oracle` (`settle_series`). The series is marked `Settled` before
//! anything else happens, and settlement itself never moves funds.
//!
//! Claims are per position and idempotent (`is_exercised` for longs,
//! `is_settled` for shorts): holders call `exercise`, writers
//! `reclaim_collateral`, and anyone may close any position for its owner
//! with `settle_position` — including worthless ones, so out-of-the-money
//! collateral is never stuck behind an absent owner.
//!
//! Longs and shorts are not matched one-to-one in this market, so each
//! side settles pro rata against the other: longs share what writers lose,
//! writers lose only what longs receive (`math::long_settlement_payout` /
//! `math::short_settlement_loss`). Every payout is drawn from the series'
//! own collateral ledger (`SeriesCollateral`), never from the premium
//! pool or another series' funds; once `escrow_settlement_to_vault` has
//! moved that collateral into the pinned vault, claims withdraw from the
//! vault tag scoped to the series (`vault_client::series_tag`).

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contractimpl, panic_with_error, token, vec, Address, BytesN, Env, IntoVal, Symbol,
    Vec,
};

mod error;
mod events;
pub mod math;
mod price_oracle_client;
pub mod storage;
pub mod types;
mod vault_client;

mod migrations;
mod params_client;
pub mod ttl;

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_adversaries;

use error::Error;
use math::{
    calc_call_collateral, calc_fee, calc_payout, calc_put_collateral, long_settlement_payout,
    max_premium, short_settlement_loss, DEFAULT_FEE_RATE_BPS, MAX_BATCH_SIZE, MAX_FEE_RATE_BPS,
    MAX_SERIES_PER_UNDERLYING, MIN_EXPIRY_LEAD, PRICE_PRECISION,
};
use storage::{
    add_open_interest, add_premium, add_user_position, fee_rate_bps, get_position, get_series,
    get_series_escrow, get_user_positions, increment_series_count, next_position_id,
    next_series_id, premium_pool, require_active_series, require_not_paused,
    series_count_for_underlying, save_position, save_series, set_premium_pool, set_series_escrow,
    total_open_interest, total_premiums,
};
use types::{DataKey, OptionPosition, OptionSeries, OptionType, PositionSide, SeriesState};
use zenith_common::{require_admin_or_multisig, ActionClass, Auth};

// ─── helpers ──────────────────────────────────────────────────────────────────

/// The single admin-auth path (#192): `Auth::Admin` requires the stored
/// admin's signature, `Auth::Multisig` an executable approval of `class`.
fn authorize(env: &Env, auth: Auth, class: ActionClass) {
    require_admin_or_multisig(env, &DataKey::Admin, &auth, class, Error::Unauthorized);
}

fn multisig(multisig_contract: Address, action_id: u64) -> Auth {
    Auth::Multisig(multisig_contract, action_id)
}

fn collateral_token(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::CollateralToken)
        .unwrap()
}

fn fee_recipient(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::FeeRecipient)
        .unwrap()
}

fn underlying_price(env: &Env, symbol: &Symbol) -> i128 {
    ttl::get_persistent(env, &DataKey::UnderlyingPrice(symbol.clone())).unwrap_or(0i128)
}

fn get_pinned_vault(env: &Env) -> Option<Address> {
    env.storage().instance().get(&DataKey::Vault)
}

fn get_series_vault_address(env: &Env, series_id: u64) -> Option<Address> {
    ttl::get_persistent(env, &DataKey::SeriesVault(series_id))
}

fn is_series_halted(env: &Env, series_id: u64) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::SeriesHalted(series_id))
}

/// `(long_open_interest, short_open_interest)` of a series.
fn side_oi(env: &Env, series_id: u64) -> (i128, i128) {
    ttl::get_persistent(env, &DataKey::SeriesSideOI(series_id)).unwrap_or((0, 0))
}

fn add_side_oi(env: &Env, series_id: u64, long: i128, short: i128) {
    let (l, s) = side_oi(env, series_id);
    ttl::set_persistent(env, &DataKey::SeriesSideOI(series_id), &(l + long, s + short));
}

fn series_collateral(env: &Env, series_id: u64) -> i128 {
    ttl::get_persistent(env, &DataKey::SeriesCollateral(series_id)).unwrap_or(0)
}

fn set_series_collateral(env: &Env, series_id: u64, amount: i128) {
    ttl::set_persistent(env, &DataKey::SeriesCollateral(series_id), &amount);
}

/// New purchases and writes need an active, unexpired, unhalted series.
fn require_tradeable(env: &Env, series: &OptionSeries) {
    if env.ledger().timestamp() >= series.expiry {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    if is_series_halted(env, series.series_id) {
        panic_with_error!(env, Error::SeriesHalted);
    }
}

/// Premium must be positive (`InvalidSeriesParams`, as at creation) and
/// at most `math::max_premium` (`PremiumOutOfBounds`).
fn validate_premium(
    env: &Env,
    underlying: &Symbol,
    option_type: &OptionType,
    strike_price: i128,
    premium: i128,
) {
    if premium <= 0 {
        panic_with_error!(env, Error::InvalidSeriesParams);
    }
    let is_call = *option_type == OptionType::Call;
    if premium > max_premium(is_call, strike_price, underlying_price(env, underlying)) {
        panic_with_error!(env, Error::PremiumOutOfBounds);
    }
}

/// A settled series and its settlement price. Rejects claims before
/// expiry (`SeriesNotExpired`) and against an unsettled series
/// (`PriceNotSet`).
fn settled_series(env: &Env, series_id: u64) -> (OptionSeries, i128) {
    let series = get_series(env, series_id);
    if env.ledger().timestamp() <= series.expiry {
        panic_with_error!(env, Error::SeriesNotExpired);
    }
    if series.state != SeriesState::Settled {
        panic_with_error!(env, Error::PriceNotSet);
    }
    let price = series
        .settlement_price
        .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));
    (series, price)
}

/// Moves `amount` of collateral token from this contract into `vault`
/// under `series_tag(series_id)`, and checks the post-condition that the
/// tag grew by exactly `amount`.
fn deposit_to_vault(env: &Env, vault: &Address, series_id: u64, amount: i128) {
    let token = collateral_token(env);
    let this = env.current_contract_address();
    // vault.deposit pulls the funds with token.transfer(this, vault, ..),
    // a non-root call that needs this contract's authorization.
    env.authorize_as_current_contract(vec![
        env,
        InvokerContractAuthEntry::Contract(SubContractInvocation {
            context: ContractContext {
                contract: token.clone(),
                fn_name: Symbol::new(env, "transfer"),
                args: (this.clone(), vault.clone(), amount).into_val(env),
            },
            sub_invocations: vec![env],
        }),
    ]);
    let client = vault_client::Client::new(env, vault);
    let tag = vault_client::series_tag(env, series_id);
    let before = client.balance_of_tagged(&token, &tag);
    let received = client.deposit(&this, &token, &tag, &amount);
    let after = client.balance_of_tagged(&token, &tag);
    if received != amount || after - before != amount {
        panic_with_error!(env, Error::EscrowPostConditionFailed);
    }
}

/// Pays `amount` of a series' funds to `to`: out of the series' vault
/// tag once its funds were escrowed there, otherwise out of this
/// contract's balance. Callers have already bounded `amount` by the
/// series' own ledger.
fn pay_out_of_series(env: &Env, series_id: u64, to: &Address, amount: i128) {
    if amount <= 0 {
        return;
    }
    let token = collateral_token(env);
    match get_series_vault_address(env, series_id) {
        Some(vault) => vault_client::Client::new(env, &vault).withdraw(
            &token,
            &vault_client::series_tag(env, series_id),
            to,
            &amount,
        ),
        None => token::Client::new(env, &token).transfer(
            &env.current_contract_address(),
            to,
            &amount,
        ),
    }
}

// ─── contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct OptionsMarket;

#[contractimpl]
impl OptionsMarket {
    // ── lifecycle ────────────────────────────────────────────────────────

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
        env.storage()
            .instance()
            .set(&DataKey::TotalPremiumsCollected, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::TotalOpenInterest, &0i128);
        env.storage().instance().set(&DataKey::PremiumPool, &0i128);
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    // ── keeper ───────────────────────────────────────────────────────────

    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    // ── admin: transfer ──────────────────────────────────────────────────

    pub fn transfer_admin(env: Env, new_admin: Address) {
        ttl::extend_instance(&env);
        Self::transfer_admin_inner(&env, Auth::Admin, new_admin);
    }

    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        ttl::extend_instance(&env);
        Self::transfer_admin_inner(&env, multisig(multisig_contract, action_id), new_admin);
    }

    fn transfer_admin_inner(env: &Env, auth: Auth, new_admin: Address) {
        authorize(env, auth, ActionClass::Critical);
        let old = zenith_common::set_admin(env, &DataKey::Admin, &new_admin);
        events::admin_transferred(env, old, new_admin);
    }

    // ── admin: pause (#192) ──────────────────────────────────────────────

    /// Contract-wide pause: blocks `create_series`, `update_premium`,
    /// `buy_option`, `write_option` (and their batch/multisig forms). Does
    /// NOT block settlement or claims — a pause winds positions down, it
    /// doesn't trap funds.
    pub fn pause(env: Env) {
        ttl::extend_instance(&env);
        Self::set_paused_inner(&env, Auth::Admin, true);
    }

    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        ttl::extend_instance(&env);
        Self::set_paused_inner(&env, multisig(multisig_contract, action_id), true);
    }

    pub fn unpause(env: Env) {
        ttl::extend_instance(&env);
        Self::set_paused_inner(&env, Auth::Admin, false);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        ttl::extend_instance(&env);
        Self::set_paused_inner(&env, multisig(multisig_contract, action_id), false);
    }

    fn set_paused_inner(env: &Env, auth: Auth, paused: bool) {
        // Stopping is an emergency action; resuming is not.
        let class = if paused {
            ActionClass::Emergency
        } else {
            ActionClass::Standard
        };
        authorize(env, auth, class);
        zenith_common::set_paused(env, &DataKey::Paused, paused);
        if paused {
            events::paused(env);
        } else {
            events::unpaused(env);
        }
    }

    // ── admin: series halt (#192) ────────────────────────────────────────

    /// Halts new purchases and writes on one Active series while an
    /// incident is investigated. Settlement, exercise and claims are
    /// unaffected. Admin only (or multisig, `Emergency` class).
    pub fn halt_series(env: Env, series_id: u64) {
        ttl::extend_instance(&env);
        Self::set_halted_inner(&env, Auth::Admin, series_id, true);
    }

    pub fn halt_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        series_id: u64,
    ) {
        ttl::extend_instance(&env);
        Self::set_halted_inner(&env, multisig(multisig_contract, action_id), series_id, true);
    }

    /// Lifts a `halt_series`. Fails with `SeriesNotHalted` if the series
    /// isn't halted. Admin only (or multisig, `Standard` class).
    pub fn unhalt_series(env: Env, series_id: u64) {
        ttl::extend_instance(&env);
        Self::set_halted_inner(&env, Auth::Admin, series_id, false);
    }

    pub fn unhalt_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        series_id: u64,
    ) {
        ttl::extend_instance(&env);
        Self::set_halted_inner(&env, multisig(multisig_contract, action_id), series_id, false);
    }

    fn set_halted_inner(env: &Env, auth: Auth, series_id: u64, halted: bool) {
        let class = if halted {
            ActionClass::Emergency
        } else {
            ActionClass::Standard
        };
        authorize(env, auth, class);
        require_active_series(env, series_id);
        let key = DataKey::SeriesHalted(series_id);
        if halted {
            ttl::set_persistent(env, &key, &true);
        } else {
            if !env.storage().persistent().has(&key) {
                panic_with_error!(env, Error::SeriesNotHalted);
            }
            env.storage().persistent().remove(&key);
        }
        events::series_halted(env, series_id, halted);
    }

    // ── admin: fee rate ──────────────────────────────────────────────────

    pub fn set_fee_rate(env: Env, new_bps: i128) {
        ttl::extend_instance(&env);
        Self::set_fee_rate_inner(&env, Auth::Admin, new_bps);
    }

    pub fn set_fee_rate_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_bps: i128,
    ) {
        ttl::extend_instance(&env);
        Self::set_fee_rate_inner(&env, multisig(multisig_contract, action_id), new_bps);
    }

    fn set_fee_rate_inner(env: &Env, auth: Auth, new_bps: i128) {
        authorize(env, auth, ActionClass::Standard);
        if !(0..=MAX_FEE_RATE_BPS).contains(&new_bps) {
            panic_with_error!(env, Error::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::FeeRateBps, &new_bps);
        events::fee_rate_updated(env, new_bps);
    }

    // ── admin: vault / price oracle ──────────────────────────────────────

    /// Pins the trusted vault address. Admin only.
    pub fn set_vault(env: Env, vault: Address) {
        ttl::extend_instance(&env);
        authorize(&env, Auth::Admin, ActionClass::Standard);
        let old = get_pinned_vault(&env);
        env.storage().instance().set(&DataKey::Vault, &vault);
        events::vault_updated(&env, old, vault);
    }

    /// Pins the `price_oracle` contract `settle_series` reads. Admin only
    /// (or multisig, `Standard` class).
    pub fn set_price_oracle(env: Env, oracle_contract: Address) {
        ttl::extend_instance(&env);
        Self::set_price_oracle_inner(&env, Auth::Admin, oracle_contract);
    }

    pub fn set_price_oracle_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        oracle_contract: Address,
    ) {
        ttl::extend_instance(&env);
        Self::set_price_oracle_inner(&env, multisig(multisig_contract, action_id), oracle_contract);
    }

    fn set_price_oracle_inner(env: &Env, auth: Auth, oracle_contract: Address) {
        authorize(env, auth, ActionClass::Standard);
        env.storage()
            .instance()
            .set(&DataKey::PriceOracle, &oracle_contract);
        events::price_oracle_updated(env, oracle_contract);
    }

    // ── admin: series management (#192) ──────────────────────────────────

    /// Creates a new option series. Admin only (or multisig).
    /// `strike_price` must be > 0 and `premium` within
    /// `(0, math::max_premium]`. Capped at `MAX_SERIES_PER_UNDERLYING` per
    /// underlying.
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
        authorize(&env, Auth::Admin, ActionClass::Standard);
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

    #[allow(clippy::too_many_arguments)]
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
        authorize(
            &env,
            multisig(multisig_contract, action_id),
            ActionClass::Standard,
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
        if expiry <= now + MIN_EXPIRY_LEAD {
            panic_with_error!(env, Error::ExpiryTooSoon);
        }
        if strike_price <= 0 || implied_vol < 0 {
            panic_with_error!(env, Error::InvalidSeriesParams);
        }
        validate_premium(env, &underlying, &option_type, strike_price, premium);
        let count = series_count_for_underlying(env, &underlying);
        if count >= MAX_SERIES_PER_UNDERLYING {
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
        save_series(env, &series);
        increment_series_count(env, &underlying);
        events::series_created(env, series_id, strike_price, expiry, premium);
        series_id
    }

    /// Sets the premium and implied vol of an Active, unexpired series.
    /// Admin only. The new premium must be within `(0, math::max_premium]`.
    ///
    /// Allowed after purchases: it only prices future purchases and
    /// writes. Existing positions keep the premium and fee they recorded
    /// at open time, and refunds are computed from those.
    pub fn update_premium(env: Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        authorize(&env, Auth::Admin, ActionClass::Standard);
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
        require_not_paused(&env);
        authorize(
            &env,
            multisig(multisig_contract, action_id),
            ActionClass::Standard,
        );
        Self::update_premium_inner(&env, series_id, new_premium, new_implied_vol);
    }

    fn update_premium_inner(env: &Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
        let mut series = require_active_series(env, series_id);
        if env.ledger().timestamp() >= series.expiry {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        if new_implied_vol < 0 {
            panic_with_error!(env, Error::InvalidSeriesParams);
        }
        validate_premium(
            env,
            &series.underlying,
            &series.option_type,
            series.strike_price,
            new_premium,
        );
        series.premium = new_premium;
        series.implied_vol = new_implied_vol;
        save_series(env, &series);
        events::premium_updated(env, series_id, new_premium, new_implied_vol);
    }

    /// Cancels an Active series. Position holders call `claim_refund`
    /// individually afterward.
    pub fn cancel_series(env: Env, series_id: u64) {
        ttl::extend_instance(&env);
        Self::cancel_series_inner(&env, Auth::Admin, series_id);
    }

    pub fn cancel_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        series_id: u64,
    ) {
        ttl::extend_instance(&env);
        Self::cancel_series_inner(&env, multisig(multisig_contract, action_id), series_id);
    }

    fn cancel_series_inner(env: &Env, auth: Auth, series_id: u64) {
        authorize(env, auth, ActionClass::Standard);
        let mut series = require_active_series(env, series_id);
        series.state = SeriesState::Cancelled;
        save_series(env, &series);
        events::series_cancelled(env, series_id);
    }

    // ── settlement (#191) ─────────────────────────────────────────────────

    /// Trusted-oracle-address flow: the address stored as `Oracle` at
    /// initialize asserts the price directly.
    pub fn set_settlement_price(env: Env, series_id: u64, price: i128) {
        ttl::extend_instance(&env);
        let oracle: Address = env.storage().instance().get(&DataKey::Oracle).unwrap();
        oracle.require_auth();
        Self::finalize_settlement(&env, series_id, price);
    }

    /// Permissionless alternative: reads the price from a live
    /// `price_oracle` deployment via cross-contract call.
    pub fn set_settlement_price_from_oracle(env: Env, series_id: u64, oracle_contract: Address) {
        ttl::extend_instance(&env);
        Self::settle_from_oracle(&env, series_id, &oracle_contract);
    }

    /// Permissionless settlement against the pinned `price_oracle`
    /// (`set_price_oracle`): once the series has expired and the oracle
    /// has a price for its underlying, anyone can settle it. Fails with
    /// `SeriesNotExpired` before expiry, `PriceNotSet` if the oracle has
    /// no fresh quorum price, `AlreadySettled` on a second call.
    pub fn settle_series(env: Env, series_id: u64) {
        ttl::extend_instance(&env);
        let oracle: Address = env
            .storage()
            .instance()
            .get(&DataKey::PriceOracle)
            .unwrap_or_else(|| panic_with_error!(&env, Error::PriceOracleNotSet));
        Self::settle_from_oracle(&env, series_id, &oracle);
    }

    fn settle_from_oracle(env: &Env, series_id: u64, oracle_contract: &Address) {
        let series = get_series(env, series_id);
        if env.ledger().timestamp() <= series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        let price = price_oracle_client::Client::new(env, oracle_contract)
            .get_price(&series.underlying)
            .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));
        Self::finalize_settlement(env, series_id, price);
    }

    /// The one place a series becomes `Settled`. Only flips state and
    /// records the price — no transfers, so no counterparty can make
    /// settlement fail. Halted series settle normally.
    fn finalize_settlement(env: &Env, series_id: u64, price: i128) {
        let mut series = get_series(env, series_id);
        if env.ledger().timestamp() <= series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        if series.state == SeriesState::Settled {
            panic_with_error!(env, Error::AlreadySettled);
        }
        if series.state != SeriesState::Active {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        if price <= 0 {
            panic_with_error!(env, Error::PriceNotSet);
        }
        series.state = SeriesState::Settled;
        series.settlement_price = Some(price);
        save_series(env, &series);
        ttl::set_persistent(
            env,
            &DataKey::UnderlyingPrice(series.underlying.clone()),
            &price,
        );
        events::settlement_price_set(env, series_id, price);
    }

    /// Permissionless: moves a settled series' remaining collateral into
    /// the pinned vault under the series tag. From then on every claim on
    /// the series withdraws from that tag. Callable once per series.
    pub fn escrow_settlement_to_vault(env: Env, series_id: u64) {
        ttl::extend_instance(&env);
        let vault = get_pinned_vault(&env)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Unauthorized));
        let series = get_series(&env, series_id);
        if series.state != SeriesState::Settled {
            panic_with_error!(&env, Error::PriceNotSet);
        }
        let amount = series_collateral(&env, series_id);
        if get_series_vault_address(&env, series_id).is_some() || amount <= 0 {
            panic_with_error!(&env, Error::NothingToEscrow);
        }
        ttl::set_persistent(&env, &DataKey::SeriesVault(series_id), &vault);
        deposit_to_vault(&env, &vault, series_id, amount);
        events::settlement_escrowed_to_vault(&env, series_id, vault, amount);
    }

    // ── traders: buy_option ───────────────────────────────────────────────

    /// Opens a Long position. `max_premium` is slippage protection.
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
        require_tradeable(env, &series);
        let total_premium = series.premium * contracts / PRICE_PRECISION;
        if total_premium > max_premium {
            panic_with_error!(env, Error::InsufficientPremium);
        }

        let fee = calc_fee(total_premium, fee_rate_bps(env));
        let net_premium = total_premium - fee;

        let tok = token::Client::new(env, &collateral_token(env));
        tok.transfer(&buyer, &env.current_contract_address(), &total_premium);
        if fee > 0 {
            tok.transfer(&env.current_contract_address(), &fee_recipient(env), &fee);
        }

        let pool = premium_pool(env) + net_premium;
        set_premium_pool(env, pool);
        add_premium(env, total_premium);
        add_open_interest(env, contracts);
        add_side_oi(env, series_id, contracts, 0);

        // Update series escrow (refund liability)
        let escrow = get_series_escrow(env, series_id) + net_premium;
        set_series_escrow(env, series_id, escrow);

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
        save_position(env, &position);
        add_user_position(env, &buyer, pos_id);

        series.open_interest += contracts;
        save_series(env, &series);

        events::option_bought(env, buyer, pos_id, series_id, contracts, total_premium);
        pos_id
    }

    /// Batch buy: up to MAX_BATCH_SIZE (series_id, contracts, max_premium) orders.
    pub fn buy_batch(env: Env, buyer: Address, orders: Vec<(u64, i128, i128)>) -> Vec<u64> {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        buyer.require_auth();
        if orders.is_empty() || orders.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut ids = Vec::new(&env);
        for (series_id, contracts, max_premium) in orders.iter() {
            let id = Self::buy_option_inner(&env, buyer.clone(), series_id, contracts, max_premium);
            ids.push_back(id);
        }
        ids
    }

    // ── traders: write_option ─────────────────────────────────────────────

    /// Opens a Short position. Writer locks collateral; receives net premium
    /// from the pool. `min_premium` is slippage protection: fails with
    /// `PremiumBelowMinimum` if the net premium the writer receives is below
    /// it. Pass `0` to opt out.
    pub fn write_option(
        env: Env,
        writer: Address,
        series_id: u64,
        contracts: i128,
        collateral_amount: i128,
        min_premium: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        writer.require_auth();
        Self::write_option_inner(
            &env,
            writer,
            series_id,
            contracts,
            collateral_amount,
            min_premium,
        )
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
        require_tradeable(env, &series);

        let spot = underlying_price(env, &series.underlying);
        let required_collateral = match series.option_type {
            OptionType::Call => calc_call_collateral(spot, series.strike_price, contracts),
            OptionType::Put => calc_put_collateral(series.strike_price, contracts),
        };
        if collateral_amount < required_collateral {
            panic_with_error!(env, Error::InsufficientCollateral);
        }

        let gross_premium = series.premium * contracts / PRICE_PRECISION;
        let fee = calc_fee(gross_premium, fee_rate_bps(env));
        let writer_premium = gross_premium - fee;

        if writer_premium < min_premium {
            panic_with_error!(env, Error::PremiumBelowMinimum);
        }

        let pool = premium_pool(env);
        if pool < writer_premium {
            panic_with_error!(env, Error::InsufficientPremiumPool);
        }

        let tok = token::Client::new(env, &collateral_token(env));
        // Pull collateral from writer
        tok.transfer(&writer, &env.current_contract_address(), &collateral_amount);
        // Pay writer their net premium from the pool
        if writer_premium > 0 {
            tok.transfer(&env.current_contract_address(), &writer, &writer_premium);
        }

        set_premium_pool(env, pool - writer_premium);
        add_open_interest(env, contracts);
        add_side_oi(env, series_id, 0, contracts);
        set_series_collateral(
            env,
            series_id,
            series_collateral(env, series_id) + collateral_amount,
        );

        // Update series escrow (collateral refund liability)
        let escrow = get_series_escrow(env, series_id) + collateral_amount;
        set_series_escrow(env, series_id, escrow);

        let pos_id = next_position_id(env);
        let position = OptionPosition {
            position_id: pos_id,
            series_id,
            owner: writer.clone(),
            side: PositionSide::Short,
            contracts,
            premium_paid: writer_premium,
            fee_paid: 0,
            collateral_locked: collateral_amount,
            is_exercised: false,
            is_settled: false,
            opened_at: env.ledger().timestamp(),
        };
        save_position(env, &position);
        add_user_position(env, &writer, pos_id);

        series.open_interest += contracts;
        save_series(env, &series);

        events::option_written(
            env,
            writer,
            pos_id,
            series_id,
            contracts,
            writer_premium,
            collateral_amount,
        );
        pos_id
    }

    /// Batch write: up to MAX_BATCH_SIZE (series_id, contracts, collateral_amount) orders.
    pub fn write_batch(env: Env, writer: Address, orders: Vec<(u64, i128, i128)>) -> Vec<u64> {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        writer.require_auth();
        if orders.is_empty() || orders.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut ids = Vec::new(&env);
        for (series_id, contracts, collateral_amount) in orders.iter() {
            let id = Self::write_option_inner(
                &env,
                writer.clone(),
                series_id,
                contracts,
                collateral_amount,
                0,
            );
            ids.push_back(id);
        }
        ids
    }

    // ── claims (#191) ─────────────────────────────────────────────────────

    /// Long-side payout after settlement. Owner only; rejects positions
    /// whose option expired worthless (`NotInTheMoney`) — close those with
    /// `settle_position`. Pays at most once per position.
    pub fn exercise(env: Env, owner: Address, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        Self::claim_long(&env, Some(&owner), position_id, true)
    }

    /// Batched `exercise`, all-or-nothing, capped at MAX_BATCH_SIZE.
    pub fn exercise_batch(env: Env, owner: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        let n = position_ids.len();
        if n == 0 || n > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut total = 0i128;
        for pos_id in position_ids.iter() {
            total += Self::claim_long(&env, Some(&owner), pos_id, true);
        }
        total
    }

    /// Short-side payout after settlement: locked collateral minus the
    /// writer's share of what the long side is owed. Owner only; pays at
    /// most once per position.
    pub fn reclaim_collateral(env: Env, writer: Address, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        writer.require_auth();
        Self::claim_short(&env, Some(&writer), position_id)
    }

    /// Batched `reclaim_collateral`, all-or-nothing, capped at MAX_BATCH_SIZE.
    pub fn reclaim_batch(env: Env, writer: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        writer.require_auth();
        let n = position_ids.len();
        if n == 0 || n > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let mut total = 0i128;
        for pos_id in position_ids.iter() {
            total += Self::claim_short(&env, Some(&writer), pos_id);
        }
        total
    }

    /// Permissionless keeper claim: closes any position on a settled
    /// series and pays whatever it is owed — possibly zero — to its
    /// owner. Lets out-of-the-money positions be closed and their
    /// collateral freed without the owner's involvement. Returns the
    /// amount paid.
    pub fn settle_position(env: Env, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        match get_position(&env, position_id).side {
            PositionSide::Long => Self::claim_long(&env, None, position_id, false),
            PositionSide::Short => Self::claim_short(&env, None, position_id),
        }
    }

    /// Shared core of every long claim. `owner = None` is the keeper path
    /// (pays the recorded owner, accepts worthless positions).
    fn claim_long(env: &Env, owner: Option<&Address>, position_id: u64, reject_otm: bool) -> i128 {
        let mut position = get_position(env, position_id);
        if let Some(owner) = owner {
            if &position.owner != owner {
                panic_with_error!(env, Error::Unauthorized);
            }
        }
        if position.side != PositionSide::Long {
            panic_with_error!(env, Error::WrongSide);
        }
        if position.is_exercised {
            panic_with_error!(env, Error::AlreadyExercised);
        }
        let (series, price) = settled_series(env, position.series_id);
        let is_call = series.option_type == OptionType::Call;
        if reject_otm && calc_payout(is_call, series.strike_price, price, position.contracts) == 0 {
            panic_with_error!(env, Error::NotInTheMoney);
        }
        let (long_oi, short_oi) = side_oi(env, series.series_id);
        let ledger = series_collateral(env, series.series_id);
        let payout = long_settlement_payout(
            is_call,
            series.strike_price,
            price,
            position.contracts,
            long_oi,
            short_oi,
        )
        .unwrap_or_else(|| panic_with_error!(env, Error::PayoffOverflow))
        .min(ledger);

        // Effects before the transfer: the position can never pay twice.
        position.is_exercised = true;
        position.is_settled = true;
        save_position(env, &position);
        set_series_collateral(env, series.series_id, ledger - payout);

        pay_out_of_series(env, series.series_id, &position.owner, payout);
        if owner.is_some() {
            events::option_exercised(env, position.owner, position_id, price, payout);
        } else {
            events::option_auto_exercised(env, position.owner, position_id, price, payout);
        }
        payout
    }

    /// Shared core of every short claim. `owner = None` is the keeper path.
    fn claim_short(env: &Env, owner: Option<&Address>, position_id: u64) -> i128 {
        let mut position = get_position(env, position_id);
        if let Some(owner) = owner {
            if &position.owner != owner {
                panic_with_error!(env, Error::Unauthorized);
            }
        }
        if position.side != PositionSide::Short {
            panic_with_error!(env, Error::WrongSide);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }
        let (series, price) = settled_series(env, position.series_id);
        let (long_oi, short_oi) = side_oi(env, series.series_id);
        let loss = short_settlement_loss(
            series.option_type == OptionType::Call,
            series.strike_price,
            price,
            position.contracts,
            position.collateral_locked,
            long_oi,
            short_oi,
        )
        .unwrap_or_else(|| panic_with_error!(env, Error::PayoffOverflow));
        let ledger = series_collateral(env, series.series_id);
        let reclaim = (position.collateral_locked - loss).max(0).min(ledger);

        position.is_settled = true;
        save_position(env, &position);
        set_series_collateral(env, series.series_id, ledger - reclaim);

        pay_out_of_series(env, series.series_id, &position.owner, reclaim);
        events::collateral_reclaimed(env, position.owner, position_id, reclaim);
        reclaim
    }

    // ── cancel / claim_refund ─────────────────────────────────────────────

    /// On a Cancelled series: longs get their net premium back; shorts get
    /// their full collateral back. Paid directly from options_market's own
    /// balance.
    pub fn claim_refund(env: Env, owner: Address, position_id: u64) {
        ttl::extend_instance(&env);
        owner.require_auth();
        let refund = Self::close_for_refund(&env, &owner, position_id);
        if refund > 0 {
            let tok = token::Client::new(&env, &collateral_token(&env));
            tok.transfer(&env.current_contract_address(), &owner, &refund);
        }
        events::refund_claimed(&env, owner, position_id, refund);
    }

    /// Permissionless: moves the remaining refund liability for a Cancelled
    /// series into the pinned vault under the series tag. Callable at most
    /// once per series (zeroes `SeriesEscrow` on success).
    pub fn escrow_series_to_vault(env: Env, vault_contract: Address, series_id: u64) {
        ttl::extend_instance(&env);
        let pinned = get_pinned_vault(&env)
            .unwrap_or_else(|| panic_with_error!(&env, Error::Unauthorized));
        if vault_contract != pinned {
            panic_with_error!(&env, Error::NotEligibleForForfeiture);
        }

        let series = get_series(&env, series_id);
        if series.state != SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotCancelled);
        }
        let escrow = get_series_escrow(&env, series_id);
        if escrow <= 0 {
            panic_with_error!(&env, Error::NothingToEscrow);
        }

        set_series_escrow(&env, series_id, 0);
        ttl::set_persistent(&env, &DataKey::SeriesVault(series_id), &vault_contract);
        deposit_to_vault(&env, &vault_contract, series_id, escrow);

        events::series_escrowed_to_vault(&env, series_id, escrow);
    }

    /// Same eligibility checks and refund formula as `claim_refund`, but
    /// pays out of the series-tagged escrow in the vault recorded by
    /// `escrow_series_to_vault` (`vault_contract` must be that vault).
    pub fn claim_refund_from_vault(
        env: Env,
        vault_contract: Address,
        owner: Address,
        position_id: u64,
    ) {
        ttl::extend_instance(&env);
        owner.require_auth();
        let series_id = get_position(&env, position_id).series_id;
        if get_series_vault_address(&env, series_id) != Some(vault_contract) {
            panic_with_error!(&env, Error::NotEligibleForForfeiture);
        }
        let refund = Self::close_for_refund(&env, &owner, position_id);
        pay_out_of_series(&env, series_id, &owner, refund);
        events::refund_claimed(&env, owner, position_id, refund);
    }

    /// Shared checks and bookkeeping of both refund paths. Marks the
    /// position settled and returns the refund owed.
    fn close_for_refund(env: &Env, owner: &Address, position_id: u64) -> i128 {
        let mut position = get_position(env, position_id);
        if &position.owner != owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }
        let series = get_series(env, position.series_id);
        if series.state != SeriesState::Cancelled {
            panic_with_error!(env, Error::SeriesNotCancelled);
        }

        let refund = match position.side {
            PositionSide::Long => position.premium_paid - position.fee_paid,
            PositionSide::Short => {
                let ledger = series_collateral(env, position.series_id);
                set_series_collateral(
                    env,
                    position.series_id,
                    (ledger - position.collateral_locked).max(0),
                );
                position.collateral_locked
            }
        };

        position.is_settled = true;
        save_position(env, &position);

        let escrow = get_series_escrow(env, position.series_id);
        set_series_escrow(env, position.series_id, escrow.saturating_sub(refund));
        refund
    }

    // ── admin: upgrade ────────────────────────────────────────────────────

    /// Swaps the contract's wasm. Admin only.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        authorize(&env, Auth::Admin, ActionClass::Critical);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Permissionless alternative to `upgrade`, authorized by a multisig.
    pub fn upgrade_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_wasm_hash: BytesN<32>,
    ) {
        ttl::extend_instance(&env);
        authorize(
            &env,
            multisig(multisig_contract, action_id),
            ActionClass::Critical,
        );
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    // ── views ──────────────────────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        zenith_common::get_admin(&env, &DataKey::Admin)
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        zenith_common::is_paused(&env, &DataKey::Paused)
    }

    pub fn is_series_halted(env: Env, series_id: u64) -> bool {
        ttl::extend_instance(&env);
        is_series_halted(&env, series_id)
    }

    pub fn get_fee_rate(env: Env) -> i128 {
        ttl::extend_instance(&env);
        fee_rate_bps(&env)
    }

    pub fn get_premium_pool(env: Env) -> i128 {
        ttl::extend_instance(&env);
        premium_pool(&env)
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
        get_user_positions(&env, &user)
    }

    pub fn get_stats(env: Env) -> (i128, i128, u64) {
        ttl::extend_instance(&env);
        let series_count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::SeriesCounter)
            .unwrap_or(0u64);
        (total_premiums(&env), total_open_interest(&env), series_count)
    }

    pub fn get_series_count_for_underlying(env: Env, symbol: Symbol) -> u32 {
        ttl::extend_instance(&env);
        series_count_for_underlying(&env, &symbol)
    }

    pub fn get_series_escrow(env: Env, series_id: u64) -> i128 {
        ttl::extend_instance(&env);
        get_series_escrow(&env, series_id)
    }

    /// `(long_open_interest, short_open_interest)` of a series.
    pub fn get_series_side_oi(env: Env, series_id: u64) -> (i128, i128) {
        ttl::extend_instance(&env);
        side_oi(&env, series_id)
    }

    /// Writer collateral still backing a series' settlement claims.
    pub fn get_series_collateral(env: Env, series_id: u64) -> i128 {
        ttl::extend_instance(&env);
        series_collateral(&env, series_id)
    }

    pub fn get_vault(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        get_pinned_vault(&env)
    }

    pub fn get_price_oracle(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::PriceOracle)
    }

    pub fn get_series_vault(env: Env, series_id: u64) -> Option<Address> {
        ttl::extend_instance(&env);
        get_series_vault_address(&env, series_id)
    }
}
