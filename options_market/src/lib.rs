#![no_std]

//! # Checks-effects-interactions checklist
//!
//! All mutating entrypoints sharing collateral/premium/settlement liabilities
//! must use one `zenith_common::guard` key. Acquire before auth (including
//! `Auth::Multisig`), oracle discovery, vault calls, and token balance reads.
//! Complete validation and checked arithmetic before effects; persist terminal
//! state and debit claims before token/vault payouts. Keep the guard until all
//! interactions finish; propagate failures as contract errors for rollback.
//! Batch methods should acquire once and call private, non-acquiring helpers.
//! Never invoke a guarded public method from another guarded method.
//!
//! Integration is pending: this checkout's entrypoint implementation is absent.

//! Zenith Protocol — Decentralized Options Market on Stellar Soroban
//!
//! Supports European-style put and call options on XLM, BTC, ETH, and SOL.
//! Premium is set by the admin (computed off-chain via Black-Scholes).
//! Writers lock collateral; buyers pay premium. Settlement at expiry via oracle.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, token, vec, Address, BytesN, Env, IntoVal, Symbol,
    Vec,
};

#[cfg(test)]
mod test;

mod error;
mod events;
mod math;
mod migrations;
mod params_client;
mod price_oracle_client;
mod storage;
mod ttl;
mod types;
mod vault_client;

use error::Error;
use math::{
    calc_fee, calc_payout, DEFAULT_FEE_RATE_BPS, FORFEITURE_WINDOW, MAX_BATCH_SIZE,
    MAX_FEE_RATE_BPS, MAX_SERIES_PER_UNDERLYING, MIN_COLLATERAL_RATIO, PRICE_PRECISION,
    RATE_PRECISION,
};
use storage::{
    add_orphaned_liability, add_user_position, claim_series_index, deduct_orphaned_liability,
    fee_rate_bps, next_position_id, require_active_series, require_not_paused, settlement_window,
};
use types::{DataKey, OptionPosition, OptionSeries, OptionType, PositionSide, SeriesState};

/// Cross-calls `multisig.is_approved(action_id)` via `invoke_contract`, so
/// this crate needs no multisig wasm to compile.
fn multisig_approved(env: &Env, multisig_contract: &Address, action_id: &BytesN<32>) -> bool {
    env.invoke_contract(
        multisig_contract,
        &Symbol::new(env, "is_approved"),
        vec![env, action_id.into_val(env)],
    )
}

fn require_expired(env: &Env, expiry: u64) {
    if env.ledger().timestamp() < expiry {
        panic_with_error!(env, Error::SeriesNotExpired);
    }
}

/// The price_oracle aggregate for `underlying`. `get_price` already
/// applies the oracle's freshness and minimum-report rules, returning None
/// when they aren't met — which fails the settlement with `PriceNotSet`.
fn oracle_price(env: &Env, oracle_contract: &Address, underlying: &Symbol) -> i128 {
    price_oracle_client::Client::new(env, oracle_contract)
        .get_price(underlying)
        .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet))
}

/// Settles an expired series at `price` — the one write path shared by
/// `set_settlement_price`, `set_settlement_price_from_oracle` and
/// `settle_expiry`, so all three produce identical state and events.
fn settle_inner(env: &Env, series: &mut OptionSeries, price: i128) {
    require_expired(env, series.expiry);
    series.settlement_price = Some(price);
    series.state = SeriesState::Settled;
    ttl::set_persistent(env, &DataKey::Series(series.series_id), series);
    ttl::set_persistent(
        env,
        &DataKey::UnderlyingPrice(series.underlying.clone()),
        &price,
    );
    events::settlement_price_set(env, series.series_id, price);
}

// ─── Contract ─────────────────────────────────────────────────────────────────

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
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Oracle, &oracle);
        env.storage()
            .instance()
            .set(&DataKey::CollateralToken, &collateral_token);
        env.storage()
            .instance()
            .set(&DataKey::FeeRecipient, &fee_recipient);
        env.storage().instance().set(&DataKey::SeriesCounter, &0u64);
        env.storage()
            .instance()
            .set(&DataKey::PositionCounter, &0u64);
        env.storage()
            .instance()
            .set(&DataKey::TotalPremiumsCollected, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::TotalOpenInterest, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::FeeRateBps, &DEFAULT_FEE_RATE_BPS);
        env.storage().instance().set(&DataKey::PremiumPool, &0i128);
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    /// Admin adjusts the protocol fee rate (basis points, 1 bps = 0.01%),
    /// capped at MAX_FEE_RATE_BPS so a compromised or careless admin can't
    /// set an absurd rate that effectively confiscates every trade.
    pub fn set_fee_rate(env: Env, new_bps: u32) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let new_bps = new_bps as i128;
        if new_bps > MAX_FEE_RATE_BPS {
            panic_with_error!(&env, Error::InvalidFeeRate);
        }

        env.storage().instance().set(&DataKey::FeeRateBps, &new_bps);
        events::fee_rate_updated(&env, new_bps);
    }

    /// Permissionless alternative to set_fee_rate: cross-calls a deployed
    /// Multisig and checks is_approved(action_id) instead of requiring
    /// the admin's own signature. Same MAX_FEE_RATE_BPS cap applies —
    /// M-of-N approval doesn't bypass the sanity check, it just replaces
    /// whose signature satisfies the auth requirement.
    pub fn set_fee_rate_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        new_bps: u32,
    ) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }

        let new_bps = new_bps as i128;
        if new_bps > MAX_FEE_RATE_BPS {
            panic_with_error!(&env, Error::InvalidFeeRate);
        }

        env.storage().instance().set(&DataKey::FeeRateBps, &new_bps);
        events::fee_rate_updated(&env, new_bps);
    }

    /// Points fee rate and settlement window at a params registry (see
    /// ../params). Clears the cached registry version so the next read
    /// refreshes from it. While a registry is set, its values override
    /// set_fee_rate on every registry version change.
    pub fn set_params_registry(env: Env, registry: Address) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        env.storage()
            .instance()
            .set(&DataKey::ParamsRegistry, &registry);
        env.storage().instance().remove(&DataKey::ParamsVersion);
    }

    pub fn get_settlement_window(env: Env) -> u64 {
        ttl::extend_instance(&env);
        settlement_window(&env)
    }

    /// Admin hands off control to a new address. Requires the CURRENT admin's
    /// signature, not the incoming one — the new admin doesn't need to do
    /// anything to receive control.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Permissionless alternative to transfer_admin: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the current admin's own signature. Same rationale as
    /// pause_via_multisig — arguably even more important here, since a
    /// single lost or compromised admin key otherwise has no recovery
    /// path at all for a contract that only ever trusted one signature.
    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        new_admin: Address,
    ) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Admin-gated contract upgrade: swaps the WASM executable behind this
    /// contract's address to whatever `new_wasm_hash` was already uploaded
    /// via the Soroban deployer, while keeping the same contract ID and all
    /// existing storage. Storage layout compatibility with the new code is
    /// the deployer's responsibility, same as any Soroban upgrade.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        zenith_common::upgrade::upgrade(
            &env,
            &DataKey::Admin,
            zenith_common::upgrade::UpgradeAuth::Admin,
            new_wasm_hash,
            Error::Unauthorized,
        );
    }

    /// Permissionless alternative to `upgrade`, authorized by the
    /// **pinned** multisig (`set_upgrade_multisig`) reporting `action_id`
    /// executable as `Critical`. Swapping the contract's executable is
    /// the single most consequential action any of these contracts can
    /// take, so the multisig is pinned rather than caller-supplied.
    pub fn upgrade_via_multisig(env: Env, action_id: BytesN<32>, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(&env);
        zenith_common::upgrade::upgrade(
            &env,
            &DataKey::Admin,
            zenith_common::upgrade::UpgradeAuth::PinnedMultisig(action_id),
            new_wasm_hash,
            Error::Unauthorized,
        );
    }

    /// Emergency stop: blocks new series creation and new trades
    /// (create_series, update_premium, buy_option, write_option). Does NOT
    /// block exercise, set_settlement_price, or reclaim_collateral — a
    /// pause should let existing positions wind down, not trap funds.
    pub fn pause(env: Env) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    /// Permissionless alternative to pause(): instead of the single
    /// admin address asserting the pause directly, anyone can trigger it
    /// once a deployed Multisig reports `action_id` as approved — no
    /// require_auth() needed, since the approval itself (M-of-N signers,
    /// checked on-chain by the Multisig) is what's actually authorizing
    /// this, not the caller triggering it. Same `action_id`-is-opaque
    /// contract as everywhere Multisig is used: it's this caller's job to
    /// pick a stable id scheme, not Multisig's or options_market's to
    /// interpret one.
    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    // ── Series Management (Admin) ─────────────────────────────────────────────

    /// Admin lists a new option series (strike + expiry + type)
    /// Premium is computed off-chain via Black-Scholes and passed in
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
        migrations::require_current(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let now = env.ledger().timestamp();
        if expiry <= now + 3600 {
            panic_with_error!(&env, Error::ExpiryTooSoon);
        }
        if strike_price <= 0 || premium <= 0 || implied_vol < 0 {
            panic_with_error!(&env, Error::InvalidSeriesParams);
        }

        let underlying_count_key = DataKey::SeriesCountForUnderlying(underlying.clone());
        let underlying_count: u32 = ttl::get_persistent(&env, &underlying_count_key).unwrap_or(0);
        if underlying_count >= MAX_SERIES_PER_UNDERLYING {
            panic_with_error!(&env, Error::TooManySeriesForUnderlying);
        }
        ttl::set_persistent(
            &env,
            &underlying_count_key,
            &(underlying_count.checked_add(1).unwrap()),
        );

        let counter: u64 = env
            .storage()
            .instance()
            .get(&DataKey::SeriesCounter)
            .unwrap();
        let series_id = counter.checked_add(1).unwrap();
        claim_series_index(
            &env,
            &underlying,
            &option_type,
            strike_price,
            expiry,
            series_id,
        );

        let series = OptionSeries {
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
        };

        ttl::set_persistent(&env, &DataKey::Series(series_id), &series);
        env.storage()
            .instance()
            .set(&DataKey::SeriesCounter, &series_id);
        storage::index_expiry(&env, &series);

        events::series_created(&env, series_id, strike_price, expiry, premium);

        series_id
    }

    /// Permissionless alternative to create_series: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the admin's own signature. Same validation and per-
    /// underlying cap apply — approval changes who can list a series,
    /// not what parameters a series may have.
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
        ttl::extend_instance(&env);
        require_not_paused(&env);
        migrations::require_current(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }

        let now = env.ledger().timestamp();
        if expiry <= now + 3600 {
            panic_with_error!(&env, Error::ExpiryTooSoon);
        }
        if strike_price <= 0 || premium <= 0 || implied_vol < 0 {
            panic_with_error!(&env, Error::InvalidSeriesParams);
        }

        let underlying_count_key = DataKey::SeriesCountForUnderlying(underlying.clone());
        let underlying_count: u32 = ttl::get_persistent(&env, &underlying_count_key).unwrap_or(0);
        if underlying_count >= MAX_SERIES_PER_UNDERLYING {
            panic_with_error!(&env, Error::TooManySeriesForUnderlying);
        }
        ttl::set_persistent(
            &env,
            &underlying_count_key,
            &(underlying_count.checked_add(1).unwrap()),
        );

        let counter: u64 = env
            .storage()
            .instance()
            .get(&DataKey::SeriesCounter)
            .unwrap();
        let series_id = counter.checked_add(1).unwrap();
        claim_series_index(
            &env,
            &underlying,
            &option_type,
            strike_price,
            expiry,
            series_id,
        );

        let series = OptionSeries {
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
        };

        ttl::set_persistent(&env, &DataKey::Series(series_id), &series);
        env.storage()
            .instance()
            .set(&DataKey::SeriesCounter, &series_id);
        storage::index_expiry(&env, &series);

        events::series_created(&env, series_id, strike_price, expiry, premium);

        series_id
    }

    /// Admin updates premium (e.g. after volatility changes)
    pub fn update_premium(env: Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        migrations::require_current(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let mut series: OptionSeries = ttl::get_persistent(&env, &DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        if series.state != SeriesState::Active {
            panic_with_error!(&env, Error::SeriesNotActive);
        }
        // A zero (or negative) premium would let buyers open positions for
        // free and writers lock collateral for nothing (#47).
        if new_premium <= 0 {
            panic_with_error!(&env, Error::InvalidSeriesParams);
        }

        series.premium = new_premium;
        series.implied_vol = new_implied_vol;
        ttl::set_persistent(&env, &DataKey::Series(series_id), &series);

        events::premium_updated(&env, series_id, new_premium, new_implied_vol);
    }

    /// Permissionless alternative to update_premium: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the admin's own signature. Same rationale as
    /// create_series_via_multisig.
    pub fn update_premium_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        series_id: u64,
        new_premium: i128,
        new_implied_vol: i128,
    ) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        migrations::require_current(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }

        let mut series: OptionSeries = ttl::get_persistent(&env, &DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        if series.state != SeriesState::Active {
            panic_with_error!(&env, Error::SeriesNotActive);
        }
        // A zero (or negative) premium would let buyers open positions for
        // free and writers lock collateral for nothing (#47).
        if new_premium <= 0 {
            panic_with_error!(&env, Error::InvalidSeriesParams);
        }

        series.premium = new_premium;
        series.implied_vol = new_implied_vol;
        ttl::set_persistent(&env, &DataKey::Series(series_id), &series);

        events::premium_updated(&env, series_id, new_premium, new_implied_vol);
    }

    /// Admin cancels an Active series (e.g. mispriced, or the underlying
    /// feed is compromised). Existing position holders then pull their own
    /// refund via claim_refund rather than the admin pushing funds to
    /// everyone in one call — Soroban charges for the resources a call
    /// touches, and an unbounded push-refund would scale badly with the
    /// number of open positions.
    pub fn cancel_series(env: Env, series_id: u64) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let mut series: OptionSeries = ttl::get_persistent(&env, &DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        if series.state != SeriesState::Active {
            panic_with_error!(&env, Error::SeriesNotActive);
        }

        series.state = SeriesState::Cancelled;
        ttl::set_persistent(&env, &DataKey::Series(series_id), &series);

        events::series_cancelled(&env, series_id);
    }

    /// Permissionless alternative to cancel_series: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the admin's own signature. Cancelling a series is
    /// disruptive to every open position in it, so gating it behind M-of-N
    /// approval (rather than a single key) is at least as warranted here
    /// as for pause.
    pub fn cancel_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        series_id: u64,
    ) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }

        let mut series: OptionSeries = ttl::get_persistent(&env, &DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        if series.state != SeriesState::Active {
            panic_with_error!(&env, Error::SeriesNotActive);
        }

        series.state = SeriesState::Cancelled;
        ttl::set_persistent(&env, &DataKey::Series(series_id), &series);

        events::series_cancelled(&env, series_id);
    }

    /// A position holder in a Cancelled series reclaims what they put in:
    /// buyers get their premium back, writers get their collateral back.
    pub fn claim_refund(env: Env, owner: Address, position_id: u64) {
        ttl::extend_instance(&env);
        owner.require_auth();

        let mut position: OptionPosition =
            ttl::get_persistent(&env, &DataKey::Position(position_id))
                .unwrap_or_else(|| panic_with_error!(&env, Error::PositionNotFound));

        if position.owner != owner {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if position.is_settled {
            panic_with_error!(&env, Error::AlreadySettled);
        }

        let series: OptionSeries = ttl::get_persistent(&env, &DataKey::Series(position.series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        if series.state != SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotCancelled);
        }

        // Longs only ever had premium_paid (gross, including the protocol
        // fee) sitting in the vault for a moment before the fee's cut was
        // sent to fee_recipient — so refunding the full gross amount here
        // would draw down other positions' vault balances. Refund net of
        // the fee actually deducted at buy time (fee_paid, not a
        // recomputation off the CURRENT fee rate, which the admin may have
        // changed since this position was opened). The fee itself isn't
        // clawed back from fee_recipient — cancellation is an admin
        // decision, not a token-contract-level guarantee we can enforce
        // retroactively.
        let refund = match position.side {
            PositionSide::Long => position
                .premium_paid
                .checked_sub(position.fee_paid)
                .unwrap(),
            PositionSide::Short => position.collateral_locked,
        };

        if refund > 0 {
            let collateral_token: Address = env
                .storage()
                .instance()
                .get(&DataKey::CollateralToken)
                .unwrap();
            let usdc = token::Client::new(&env, &collateral_token);
            usdc.transfer(&env.current_contract_address(), &owner, &refund);
        }

        Self::debit_series_escrow(&env, position.series_id, refund);

        position.is_settled = true;
        ttl::set_persistent(&env, &DataKey::Position(position_id), &position);

        events::refund_claimed(&env, owner, position_id, refund);
    }

    /// Moves a Cancelled series' entire remaining refund liability — every
    /// not-yet-claimed position's own premium-net-of-fee or full
    /// collateral, tracked incrementally in SeriesEscrow since this
    /// series' first buy_option/write_option — out of options_market's
    /// own undifferentiated token balance and into `vault`, tagged by
    /// `series_id`. This is what actually closes the gap documented in
    /// the README: today, a writer's collateral refund and a buyer's
    /// premium refund both draw from the SAME shared contract balance
    /// that every OTHER series' positions draw from too, so nothing stops
    /// this series' claims from collectively drawing down funds that were
    /// never this series' to begin with. Once quarantined here, vault's
    /// own InsufficientEscrowBalance check enforces that claim_refund_-
    /// from_vault against this tag can never pay out more than this
    /// series specifically was ever owed — a real, provable bound instead
    /// of an assumption about the shared pot happening to have enough.
    ///
    /// Permissionless — this only relocates the contract's OWN funds into
    /// a vault it already trusts (the same "nothing external to vouch
    /// for" reasoning as set_settlement_price_from_oracle), it doesn't
    /// authorize paying out to anyone. Callable at most once per series:
    /// SeriesEscrow is zeroed here, so a second call finds nothing left
    /// to move and rejects with NothingToEscrow. Safe to call after some
    /// positions already used the original claim_refund path — SeriesEscrow
    /// is debited there too, so this only ever moves what's genuinely
    /// still outstanding.
    ///
    /// Requires `vault` to have this contract registered as an
    /// integrator (so it can create its `(self, "series", id)` tag) and
    /// the collateral token allowlisted. This contract then owns that
    /// tag, which is what lets its cross-contract `deposit`/`withdraw`
    /// calls satisfy vault's tag-owner auth checks without a human
    /// signature.
    pub fn escrow_series_to_vault(env: Env, vault_contract: Address, series_id: u64) {
        ttl::extend_instance(&env);
        let series: OptionSeries = ttl::get_persistent(&env, &DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));
        if series.state != SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotCancelled);
        }

        let series_escrow_key = DataKey::SeriesEscrow(series_id);
        let amount: i128 = ttl::get_persistent(&env, &series_escrow_key).unwrap_or(0);
        if amount <= 0 {
            panic_with_error!(&env, Error::NothingToEscrow);
        }
        ttl::set_persistent(&env, &series_escrow_key, &0i128);

        let token: Address = env
            .storage()
            .instance()
            .get(&DataKey::CollateralToken)
            .unwrap();
        let vault = vault_client::Client::new(&env, &vault_contract);
        vault.deposit(
            &env.current_contract_address(),
            &token,
            &vault_client::series_tag(&env, series_id),
            &amount,
        );

        events::series_escrowed_to_vault(&env, series_id, amount);
    }

    /// Alternative to claim_refund for a Cancelled series whose refund
    /// liability has been moved into `vault` via
    /// escrow_series_to_vault: pays this position's own
    /// refund out of vault's series_id-tagged escrow instead of
    /// options_market's own balance. Same eligibility checks and same
    /// refund formula as claim_refund — this only changes WHERE the
    /// payout is funded from, not who's entitled to what.
    pub fn claim_refund_from_vault(
        env: Env,
        vault_contract: Address,
        owner: Address,
        position_id: u64,
    ) {
        ttl::extend_instance(&env);
        owner.require_auth();

        let mut position: OptionPosition =
            ttl::get_persistent(&env, &DataKey::Position(position_id))
                .unwrap_or_else(|| panic_with_error!(&env, Error::PositionNotFound));

        if position.owner != owner {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if position.is_settled {
            panic_with_error!(&env, Error::AlreadySettled);
        }

        let series: OptionSeries = ttl::get_persistent(&env, &DataKey::Series(position.series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        if series.state != SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotCancelled);
        }

        let refund = match position.side {
            PositionSide::Long => position
                .premium_paid
                .checked_sub(position.fee_paid)
                .unwrap(),
            PositionSide::Short => position.collateral_locked,
        };

        if refund > 0 {
            let token: Address = env
                .storage()
                .instance()
                .get(&DataKey::CollateralToken)
                .unwrap();
            let vault = vault_client::Client::new(&env, &vault_contract);
            vault.withdraw(
                &token,
                &vault_client::series_tag(&env, position.series_id),
                &owner,
                &refund,
            );
        }

        Self::debit_series_escrow(&env, position.series_id, refund);

        position.is_settled = true;
        ttl::set_persistent(&env, &DataKey::Position(position_id), &position);

        events::refund_claimed(&env, owner, position_id, refund);
    }

    fn debit_series_escrow(env: &Env, series_id: u64, amount: i128) {
        let key = DataKey::SeriesEscrow(series_id);
        let outstanding: i128 = ttl::get_persistent(env, &key).unwrap_or(0);
        ttl::set_persistent(env, &key, &outstanding.checked_sub(amount).unwrap());
    }

    pub fn get_series_escrow(env: Env, series_id: u64) -> i128 {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::SeriesEscrow(series_id)).unwrap_or(0)
    }

    // ── Buying Options (Long) ─────────────────────────────────────────────────

    /// Buy `contracts` options in a series (pay premium in USDC)
    /// `contracts` is in PRICE_PRECISION scale (1 contract = 1e7)
    /// `max_premium` is slippage protection
    pub fn buy_option(
        env: Env,
        buyer: Address,
        series_id: u64,
        contracts: i128,
        max_premium: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        migrations::require_current(&env);
        buyer.require_auth();

        if contracts <= 0 {
            panic_with_error!(&env, Error::ZeroContracts);
        }

        let mut series: OptionSeries = require_active_series(&env, series_id);

        let total_premium = contracts
            .checked_mul(series.premium)
            .unwrap()
            .checked_div(PRICE_PRECISION)
            .unwrap();

        if total_premium > max_premium {
            panic_with_error!(&env, Error::InsufficientPremium);
        }

        // Collect premium
        let collateral_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::CollateralToken)
            .unwrap();
        let usdc = token::Client::new(&env, &collateral_token);

        // Protocol fee
        let fee = calc_fee(total_premium, fee_rate_bps(&env));
        let premium_after_fee = total_premium - fee;

        // Premium goes to vault (covers writer payouts on exercise)
        usdc.transfer(&buyer, &env.current_contract_address(), &total_premium);

        // Fee to protocol
        if fee > 0 {
            let fee_recipient: Address = env
                .storage()
                .instance()
                .get(&DataKey::FeeRecipient)
                .unwrap();
            usdc.transfer(&env.current_contract_address(), &fee_recipient, &fee);
        }

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

        ttl::set_persistent(&env, &DataKey::Position(pos_id), &position);
        add_user_position(&env, &buyer, pos_id);

        // Update OI
        series.open_interest = series.open_interest.checked_add(contracts).unwrap();
        ttl::set_persistent(&env, &DataKey::Series(series_id), &series);

        let total_collected: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalPremiumsCollected)
            .unwrap_or(0);
        env.storage().instance().set(
            &DataKey::TotalPremiumsCollected,
            &(total_collected.checked_add(premium_after_fee).unwrap()),
        );

        // Funds the pool writers get paid out of — see write_option for why
        // this indirection exists instead of writers drawing directly off
        // the vault's raw token balance.
        let pool: i128 = env
            .storage()
            .instance()
            .get(&DataKey::PremiumPool)
            .unwrap_or(0);
        env.storage().instance().set(
            &DataKey::PremiumPool,
            &(pool.checked_add(premium_after_fee).unwrap()),
        );

        // Tracks this position's own refund-eligible principal against
        // its series — see SeriesEscrow's doc comment and
        // escrow_series_to_vault.
        let series_escrow_key = DataKey::SeriesEscrow(series_id);
        let series_escrow: i128 = ttl::get_persistent(&env, &series_escrow_key).unwrap_or(0);
        ttl::set_persistent(
            &env,
            &series_escrow_key,
            &(series_escrow.checked_add(premium_after_fee).unwrap()),
        );

        events::option_bought(&env, buyer, pos_id, series_id, contracts, total_premium);

        pos_id
    }

    // ── Writing Options (Short / Covered) ─────────────────────────────────────

    /// Write (sell) options — lock collateral, receive premium
    /// For calls: collateral = contracts × underlying price (covered call)
    /// For puts:  collateral = contracts × strike price × 110% (cash-secured put)
    /// `min_premium` is slippage protection: the write fails with
    /// `PremiumBelowMinimum` if the net premium the writer would receive
    /// (after the protocol fee) is below it, so an `update_premium` cut or a
    /// `set_fee_rate` increase ordered ahead of the write can't lock the
    /// writer's collateral for less than they agreed to. Pass 0 to opt out.
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
        migrations::require_current(&env);
        writer.require_auth();

        if contracts <= 0 {
            panic_with_error!(&env, Error::ZeroContracts);
        }

        let mut series: OptionSeries = require_active_series(&env, series_id);

        // Required collateral depends on option type
        let required_collateral = match series.option_type {
            // Covered call: lock collateral equal to notional (underlying * contracts)
            OptionType::Call => {
                let underlying_price: i128 =
                    ttl::get_persistent(&env, &DataKey::UnderlyingPrice(series.underlying.clone()))
                        .unwrap_or(series.strike_price);
                contracts
                    .checked_mul(underlying_price)
                    .unwrap()
                    .checked_div(PRICE_PRECISION)
                    .unwrap()
            }
            // Cash-secured put: lock strike × contracts × 110%
            OptionType::Put => {
                let nominal = contracts
                    .checked_mul(series.strike_price)
                    .unwrap()
                    .checked_div(PRICE_PRECISION)
                    .unwrap();
                nominal
                    .checked_mul(MIN_COLLATERAL_RATIO)
                    .unwrap()
                    .checked_div(RATE_PRECISION)
                    .unwrap()
            }
        };

        if collateral_amount < required_collateral {
            panic_with_error!(&env, Error::InsufficientCollateral);
        }

        let collateral_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::CollateralToken)
            .unwrap();
        let usdc = token::Client::new(&env, &collateral_token);

        // Writer locks collateral
        usdc.transfer(
            &writer,
            &env.current_contract_address(),
            &required_collateral,
        );

        // Writer receives premium, drawn from the pool that buyers' premium
        // payments fund (see buy_option) — NOT from the vault's raw token
        // balance. Paying straight out of the vault would let a writer's
        // own just-deposited collateral fund their "premium," which isn't
        // a premium at all (nobody has actually paid one yet) and leaves
        // the vault unable to return that writer's full collateral later.
        // A write that arrives before enough buyer premium exists to cover
        // it is correctly rejected rather than quietly settled from funds
        // that were never earned.
        let total_premium = contracts
            .checked_mul(series.premium)
            .unwrap()
            .checked_div(PRICE_PRECISION)
            .unwrap();
        let fee = calc_fee(total_premium, fee_rate_bps(&env));
        let writer_premium = total_premium - fee;

        if writer_premium < min_premium {
            panic_with_error!(&env, Error::PremiumBelowMinimum);
        }

        let pool: i128 = env
            .storage()
            .instance()
            .get(&DataKey::PremiumPool)
            .unwrap_or(0);
        if pool < writer_premium {
            panic_with_error!(&env, Error::InsufficientPremiumPool);
        }
        env.storage().instance().set(
            &DataKey::PremiumPool,
            &(pool.checked_sub(writer_premium).unwrap()),
        );

        usdc.transfer(&env.current_contract_address(), &writer, &writer_premium);

        let pos_id = next_position_id(&env);
        let position = OptionPosition {
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
        };

        ttl::set_persistent(&env, &DataKey::Position(pos_id), &position);
        add_user_position(&env, &writer, pos_id);

        series.open_interest = series.open_interest.checked_add(contracts).unwrap();
        ttl::set_persistent(&env, &DataKey::Series(series_id), &series);

        let series_escrow_key = DataKey::SeriesEscrow(series_id);
        let series_escrow: i128 = ttl::get_persistent(&env, &series_escrow_key).unwrap_or(0);
        ttl::set_persistent(
            &env,
            &series_escrow_key,
            &(series_escrow.checked_add(required_collateral).unwrap()),
        );

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

    // ── Position Transfer ─────────────────────────────────────────────────────

    /// Transfer ownership of an open position from `from` to `to` (issue #51).
    /// `from` must sign and own the position. Both parties' `UserPositions`
    /// lists are updated, and the new owner inherits every right attached to
    /// the position (exercise for longs, collateral reclaim and cancellation
    /// refunds for either side).
    ///
    /// Shorts are transferable too: their collateral is fully locked in this
    /// contract at write time, so the recipient receives only the right to
    /// reclaim whatever collateral is left after settlement, not an unfunded
    /// obligation.
    ///
    /// Fails with `Unauthorized` if `from` isn't the owner, and with
    /// `AlreadyExercised` / `AlreadySettled` for a closed position. Blocked
    /// while paused. Transferring to `from` itself is a no-op.
    pub fn transfer_position(env: Env, from: Address, to: Address, position_id: u64) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        migrations::require_current(&env);
        from.require_auth();

        let key = DataKey::Position(position_id);
        let mut position: OptionPosition = ttl::get_persistent(&env, &key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::PositionNotFound));
        if position.owner != from {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if position.is_exercised {
            panic_with_error!(&env, Error::AlreadyExercised);
        }
        if position.is_settled {
            panic_with_error!(&env, Error::AlreadySettled);
        }
        if from == to {
            return;
        }

        let from_key = DataKey::UserPositions(from.clone());
        let mut from_positions: Vec<u64> =
            ttl::get_persistent(&env, &from_key).unwrap_or_else(|| Vec::new(&env));
        if let Some(i) = from_positions.first_index_of(position_id) {
            from_positions.remove(i);
        }
        ttl::set_persistent(&env, &from_key, &from_positions);
        add_user_position(&env, &to, position_id);

        position.owner = to.clone();
        ttl::set_persistent(&env, &key, &position);
        events::position_transferred(&env, position_id, from, to);
    }

    // ── Exercise ──────────────────────────────────────────────────────────────

    /// Exercise a long position before or at expiry (European = only at expiry)
    /// Settlement price must be set by oracle first
    pub fn exercise(env: Env, owner: Address, position_id: u64) {
        ttl::extend_instance(&env);
        owner.require_auth();
        let collateral_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::CollateralToken)
            .unwrap();
        let usdc = token::Client::new(&env, &collateral_token);
        Self::exercise_one(&env, &owner, position_id, &usdc);
    }

    /// Same as exercise(), but for every position_id in one call — for an
    /// owner with several long positions (e.g. bought into the same series
    /// more than once) who'd otherwise need one transaction per position to
    /// wind them all down. All-or-nothing, same as any other Soroban call:
    /// if any single position_id in the batch fails exercise()'s own
    /// checks (not in the money, already exercised, wrong owner, ...), the
    /// whole batch aborts rather than silently skipping it. Capped at
    /// MAX_BATCH_SIZE so the batch itself can't be sized to blow through
    /// this call's resource budget. Returns the summed payout.
    pub fn exercise_batch(env: Env, owner: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        if position_ids.is_empty() || position_ids.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let collateral_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::CollateralToken)
            .unwrap();
        let usdc = token::Client::new(&env, &collateral_token);

        let mut total_payout: i128 = 0;
        for position_id in position_ids.iter() {
            let payout = Self::exercise_one(&env, &owner, position_id, &usdc);
            total_payout = total_payout.checked_add(payout).unwrap();
        }
        total_payout
    }

    /// Permissionless keeper settlement and auto-exercise for in-the-money longs.
    /// Can be called by keepers, bots, or any third party after settlement.
    /// Payout is unconditionally transferred directly to the position owner.
    pub fn settle_long(env: Env, position_id: u64) -> i128 {
        require_not_paused(&env);
        migrations::require_current(&env);
        let collateral_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::CollateralToken)
            .unwrap();
        let usdc = token::Client::new(&env, &collateral_token);

        let position: OptionPosition = env
            .storage()
            .persistent()
            .get(&DataKey::Position(position_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::PositionNotFound));

        let payout = Self::exercise_one(&env, &position.owner, position_id, &usdc);

        let series: OptionSeries = env
            .storage()
            .persistent()
            .get(&DataKey::Series(position.series_id))
            .unwrap();
        let settlement_price = series.settlement_price.unwrap_or(0);
        events::option_auto_exercised(&env, position.owner, position_id, settlement_price, payout);

        payout
    }

    /// Shared core of exercise()/exercise_batch(): validates one position,
    /// pays out its intrinsic value, marks it exercised, and returns the
    /// payout. Assumes `owner`'s authorization was already checked by the
    /// caller — checking it once per batch, not once per position, is the
    /// entire point of exercise_batch existing.
    fn exercise_one(env: &Env, owner: &Address, position_id: u64, usdc: &token::Client) -> i128 {
        let mut position: OptionPosition =
            ttl::get_persistent(env, &DataKey::Position(position_id))
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

        let series: OptionSeries = ttl::get_persistent(env, &DataKey::Series(position.series_id))
            .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));

        let now = env.ledger().timestamp();

        // European: exercise only after expiry; soft window until forfeiture deadline
        if now < series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        if now > series.expiry + settlement_window(env) {
            panic_with_error!(env, Error::ExerciseWindowClosed);
        }

        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));

        // Determine payout
        let payout = calc_payout(
            &series.option_type,
            series.strike_price,
            settlement_price,
            position.contracts,
        );

        if payout <= 0 {
            panic_with_error!(env, Error::NotInTheMoney);
        }

        // Pay out to option holder
        usdc.transfer(&env.current_contract_address(), owner, &payout);

        position.is_exercised = true;
        ttl::set_persistent(env, &DataKey::Position(position_id), &position);

        deduct_orphaned_liability(env, payout);

        events::option_exercised(env, owner.clone(), position_id, settlement_price, payout);
        payout
    }

    /// Oracle sets the settlement price for a series
    pub fn set_settlement_price(env: Env, series_id: u64, price: i128) {
        ttl::extend_instance(&env);
        let oracle: Address = env.storage().instance().get(&DataKey::Oracle).unwrap();
        oracle.require_auth();

        let mut series: OptionSeries = ttl::get_persistent(&env, &DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        settle_inner(&env, &mut series, price);
    }

    /// Permissionless alternative to set_settlement_price: instead of a
    /// trusted `oracle` address asserting a price, anyone can settle a
    /// series once it's expired by pointing at a live price_oracle
    /// contract deployment and letting IT supply the price via a
    /// cross-contract call. No require_auth needed — the price itself is
    /// already backed by that contract's own feeder-authenticated
    /// aggregate, so there's nothing left for a caller to vouch for.
    pub fn set_settlement_price_from_oracle(env: Env, series_id: u64, oracle_contract: Address) {
        ttl::extend_instance(&env);
        let mut series: OptionSeries = ttl::get_persistent(&env, &DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        require_expired(&env, series.expiry);
        let price = oracle_price(&env, &oracle_contract, &series.underlying);
        settle_inner(&env, &mut series, price);
    }

    // ── Expiry calendar / batched settlement ─────────────────────────────────

    /// Admin pins the price_oracle contract `settle_expiry` reads. Unlike
    /// `set_settlement_price_from_oracle`, the batch path never takes an
    /// oracle from its (permissionless) caller.
    pub fn set_price_oracle(env: Env, oracle_contract: Address) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::PriceOracle, &oracle_contract);
    }

    pub fn get_price_oracle(env: Env) -> Option<Address> {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::PriceOracle)
    }

    /// Admin adds an existing series to the expiry calendar. Series are
    /// indexed automatically by `create_series`; this backfills ones listed
    /// before the calendar existed. Idempotent.
    pub fn register_expiry(env: Env, series_id: u64) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        let series: OptionSeries = ttl::get_persistent(&env, &DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));
        storage::index_expiry(&env, &series);
    }

    /// How many series are listed for `underlying` at `expiry`.
    pub fn get_expiry_count(env: Env, underlying: Symbol, expiry: u64) -> u32 {
        ttl::extend_instance(&env);
        storage::expiry_count(&env, &underlying, expiry)
    }

    /// Series ids listed for `underlying` at `expiry`, `[cursor, cursor +
    /// limit)` in listing order.
    pub fn get_expiry_series(
        env: Env,
        underlying: Symbol,
        expiry: u64,
        cursor: u32,
        limit: u32,
    ) -> Vec<u64> {
        ttl::extend_instance(&env);
        storage::require_expiry_limit(&env, limit);
        storage::expiry_page(&env, &underlying, expiry, cursor, limit)
    }

    /// Permissionless: settles up to `limit` (at most
    /// `MAX_EXPIRY_BATCH`) unsettled series listed for `underlying` at
    /// `expiry`, starting at `cursor`, against one read of the pinned
    /// price oracle. Settled and cancelled series are skipped, so repeated
    /// or overlapping calls are safe. Returns the next cursor; it equals
    /// `get_expiry_count` once the calendar is exhausted (a cursor past
    /// the end returns it immediately). If the oracle has no fresh
    /// aggregate the whole call fails and nothing is settled.
    pub fn settle_expiry(
        env: Env,
        underlying: Symbol,
        expiry: u64,
        cursor: u32,
        limit: u32,
    ) -> u32 {
        ttl::extend_instance(&env);
        storage::require_expiry_limit(&env, limit);
        require_expired(&env, expiry);
        let count = storage::expiry_count(&env, &underlying, expiry);
        if cursor >= count {
            return count;
        }
        let oracle_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::PriceOracle)
            .unwrap_or_else(|| panic_with_error!(&env, Error::PriceOracleNotSet));
        let price = oracle_price(&env, &oracle_contract, &underlying);

        let ids = storage::expiry_page(&env, &underlying, expiry, cursor, limit);
        let mut settled = 0u32;
        for series_id in ids.iter() {
            let key = DataKey::Series(series_id);
            let Some(mut series) = ttl::get_persistent::<_, OptionSeries>(&env, &key) else {
                continue;
            };
            if series.state == SeriesState::Settled || series.state == SeriesState::Cancelled {
                continue;
            }
            settle_inner(&env, &mut series, price);
            settled += 1;
        }
        let next = cursor + ids.len();
        events::expiry_settled(&env, underlying, expiry, cursor, next, settled, price);
        next
    }

    /// Writers reclaim unused collateral after settlement
    pub fn reclaim_collateral(env: Env, writer: Address, position_id: u64) {
        ttl::extend_instance(&env);
        writer.require_auth();
        let collateral_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::CollateralToken)
            .unwrap();
        let usdc = token::Client::new(&env, &collateral_token);
        Self::reclaim_one(&env, &writer, position_id, &usdc);
    }

    /// Same as reclaim_collateral(), but for every position_id in one
    /// call — same rationale and same all-or-nothing/MAX_BATCH_SIZE
    /// contract as exercise_batch(). Returns the summed reclaim.
    pub fn reclaim_batch(env: Env, writer: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        writer.require_auth();
        if position_ids.is_empty() || position_ids.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, Error::InvalidBatchSize);
        }
        let collateral_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::CollateralToken)
            .unwrap();
        let usdc = token::Client::new(&env, &collateral_token);

        let mut total_reclaim: i128 = 0;
        for position_id in position_ids.iter() {
            let reclaim = Self::reclaim_one(&env, &writer, position_id, &usdc);
            total_reclaim = total_reclaim.checked_add(reclaim).unwrap();
        }
        total_reclaim
    }

    /// Shared core of reclaim_collateral()/reclaim_batch(). Assumes
    /// `writer`'s authorization was already checked once by the caller.
    fn reclaim_one(env: &Env, writer: &Address, position_id: u64, usdc: &token::Client) -> i128 {
        let mut position: OptionPosition =
            ttl::get_persistent(env, &DataKey::Position(position_id))
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

        let series: OptionSeries = ttl::get_persistent(env, &DataKey::Series(position.series_id))
            .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));

        if series.state != SeriesState::Settled {
            panic_with_error!(env, Error::SeriesNotExpired);
        }

        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));

        // Compute how much of collateral was consumed by exercised long positions
        let max_loss = calc_payout(
            &series.option_type,
            series.strike_price,
            settlement_price,
            position.contracts,
        );
        let reclaim = position
            .collateral_locked
            .checked_sub(max_loss)
            .unwrap()
            .max(0);

        if reclaim > 0 {
            usdc.transfer(&env.current_contract_address(), writer, &reclaim);
        }

        if max_loss > 0 {
            add_orphaned_liability(env, max_loss);
        }

        position.is_settled = true;
        ttl::set_persistent(env, &DataKey::Position(position_id), &position);

        events::collateral_reclaimed(env, writer.clone(), position_id, reclaim);
        reclaim
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    pub fn get_fee_rate(env: Env) -> i128 {
        ttl::extend_instance(&env);
        fee_rate_bps(&env)
    }

    pub fn get_premium_pool(env: Env) -> i128 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::PremiumPool)
            .unwrap_or(0)
    }

    pub fn get_series_count_for_underlying(env: Env, underlying: Symbol) -> u32 {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::SeriesCountForUnderlying(underlying)).unwrap_or(0)
    }

    /// Series id listed for this exact (underlying, option_type, strike,
    /// expiry) spec, or `None` if no such series exists.
    pub fn get_series_id(
        env: Env,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
    ) -> Option<u64> {
        env.storage().persistent().get(&DataKey::SeriesIndex(
            underlying,
            option_type,
            strike_price,
            expiry,
        ))
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

    pub fn get_stats(env: Env) -> (i128, i128, u64) {
        ttl::extend_instance(&env);
        let premiums: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalPremiumsCollected)
            .unwrap_or(0);
        let oi: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalOpenInterest)
            .unwrap_or(0);
        let series_count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::SeriesCounter)
            .unwrap_or(0);
        (premiums, oi, series_count)
    }

    /// Sweeps unexercised ITM long payout after the 90-day forfeiture window
    /// to the protocol treasury/fee recipient with a payout_forfeited event.
    pub fn sweep_forfeited(env: Env, position_id: u64) -> i128 {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let mut position: OptionPosition = env
            .storage()
            .persistent()
            .get(&DataKey::Position(position_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::PositionNotFound));

        if position.side != PositionSide::Long {
            panic_with_error!(&env, Error::WrongSide);
        }
        if position.is_exercised {
            panic_with_error!(&env, Error::AlreadyExercised);
        }

        let series: OptionSeries = env
            .storage()
            .persistent()
            .get(&DataKey::Series(position.series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        if series.state != SeriesState::Settled {
            panic_with_error!(&env, Error::SeriesNotExpired);
        }

        let now = env.ledger().timestamp();
        if now <= series.expiry + FORFEITURE_WINDOW {
            panic_with_error!(&env, Error::NotEligibleForForfeiture);
        }

        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(&env, Error::PriceNotSet));

        let payout = calc_payout(
            &series.option_type,
            series.strike_price,
            settlement_price,
            position.contracts,
        );

        if payout > 0 {
            let collateral_token: Address = env
                .storage()
                .instance()
                .get(&DataKey::CollateralToken)
                .unwrap();
            let fee_recipient: Address = env
                .storage()
                .instance()
                .get(&DataKey::FeeRecipient)
                .unwrap_or_else(|| admin.clone());
            let usdc = token::Client::new(&env, &collateral_token);

            usdc.transfer(&env.current_contract_address(), &fee_recipient, &payout);
        }

        position.is_exercised = true;
        env.storage()
            .persistent()
            .set(&DataKey::Position(position_id), &position);

        deduct_orphaned_liability(&env, payout);
        events::payout_forfeited(&env, position.owner.clone(), position_id, payout);

        payout
    }

    /// Running total of unexercised ITM obligations across settled positions
    pub fn get_orphaned_liabilities(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::OrphanedLiabilities)
            .unwrap_or(0)
    }

    /// Admin entrypoint to initialize/synchronize historical orphaned liabilities
    pub fn sync_orphaned_liabilities(env: Env, amount: i128) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::OrphanedLiabilities, &amount);
    }
}
