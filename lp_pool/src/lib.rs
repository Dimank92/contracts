#![no_std]

//! Zenith LP Pool — pooled option writing on behalf of depositors.
//!
//! Users deposit the collateral asset (USDC) and receive pool shares. A
//! keeper writes options from the pool's idle assets into governance-
//! whitelisted `options_market` series, within per-series and pool-wide
//! utilization caps, and the pool earns the premiums.
//!
//! **Epochs.** `deposit` and `request_withdraw` never price against the
//! live pool: they queue into the current epoch. Once every position the
//! pool opened has settled and its collateral been reclaimed, the keeper
//! calls `process_epoch`, which fixes one share price for the epoch and
//! converts every queued deposit and withdrawal at it. Users then `claim`
//! their shares or assets. Because pricing only happens with no open
//! positions, nobody can time an entry or exit around a settlement.
//!
//! **Share price.** `total_assets = idle + locked − expected_liabilities`.
//! Premiums are paid up front by the market, so accrued premium is
//! already inside `idle`. `expected_liabilities` is the keeper's
//! mark-to-market of open positions (e.g. oracle intrinsic value) and
//! only feeds the informational `share_price` view mid-epoch. At
//! `process_epoch` there are no open positions, so `locked` and
//! liabilities are both realized: a position that paid out more than its
//! premium came back short in `reclaim`, which lowered `idle` and so the
//! share price. See docs/lp_pool.md.
//!
//! **Inflation attack.** Assets are tracked internally (a token donation
//! doesn't change `idle`), conversions use a virtual offset of
//! `VIRTUAL_SHARES` shares and `VIRTUAL_ASSETS` assets, and deposits
//! below `MIN_DEPOSIT` are rejected.

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contractimpl, token, vec, Address, Env, IntoVal, Symbol, Vec,
};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_events;

mod error;
mod events;
mod ttl;
mod types;

pub use error::Error;
pub use events::{
    ClaimedEvent, CollateralReclaimedEvent, DepositQueuedEvent, EpochProcessedEvent,
    InitializedEvent, KeeperSetEvent, LiabilitiesMarkedEvent, MaxUtilizationSetEvent,
    OptionWrittenEvent, SeriesCapSetEvent, WithdrawQueuedEvent,
};
use types::DataKey;
pub use types::{EpochResult, PoolPosition, Ticket};

zenith_common::contract_meta!();

/// Virtual shares/assets in every conversion (ERC-4626 decimal offset).
pub const VIRTUAL_SHARES: i128 = 1_000;
pub const VIRTUAL_ASSETS: i128 = 1;
/// Smallest accepted deposit, in asset base units.
pub const MIN_DEPOSIT: i128 = 1_000;
pub const BPS: i128 = 10_000;
/// Bounds `process_epoch`'s precondition scan and the position list.
pub const MAX_POSITIONS: u32 = 50;
/// Fixed-point scale of `share_price`.
pub const PRICE_SCALE: i128 = 10_000_000;

#[contract]
pub struct LpPool;

#[contractimpl]
impl LpPool {
    pub fn initialize(
        env: Env,
        admin: Address,
        keeper: Address,
        asset: Address,
        market: Address,
        max_utilization_bps: u32,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        check_bps(max_utilization_bps)?;
        ttl::extend_instance(&env);
        let s = env.storage().instance();
        s.set(&DataKey::Admin, &admin);
        s.set(&DataKey::Keeper, &keeper);
        s.set(&DataKey::Asset, &asset);
        s.set(&DataKey::Market, &market);
        s.set(&DataKey::MaxUtilizationBps, &max_utilization_bps);
        s.set(&DataKey::Epoch, &0u32);
        events::initialized(
            &env,
            InitializedEvent {
                admin,
                keeper,
                asset,
                market,
                max_utilization_bps,
            },
        );
        Ok(())
    }

    // ── Governance ────────────────────────────────────────────────────────────

    pub fn set_keeper(env: Env, keeper: Address) -> Result<(), Error> {
        require_admin(&env)?;
        env.storage().instance().set(&DataKey::Keeper, &keeper);
        events::keeper_set(&env, KeeperSetEvent { keeper });
        Ok(())
    }

    /// Cap on `locked / (idle + locked)` after a write, in bps.
    pub fn set_max_utilization(env: Env, max_utilization_bps: u32) -> Result<(), Error> {
        require_admin(&env)?;
        check_bps(max_utilization_bps)?;
        env.storage()
            .instance()
            .set(&DataKey::MaxUtilizationBps, &max_utilization_bps);
        events::max_utilization_set(
            &env,
            MaxUtilizationSetEvent {
                max_utilization_bps,
            },
        );
        Ok(())
    }

    /// Whitelists `series_id` with a collateral cap; zero de-lists it
    /// (open positions are unaffected and can still be reclaimed).
    pub fn set_series_cap(env: Env, series_id: u64, max_collateral: i128) -> Result<(), Error> {
        require_admin(&env)?;
        if max_collateral < 0 {
            return Err(Error::InvalidAmount);
        }
        ttl::set_persistent(&env, &DataKey::SeriesCap(series_id), &max_collateral);
        events::series_cap_set(&env, series_id, SeriesCapSetEvent { max_collateral });
        Ok(())
    }

    // ── Depositors ────────────────────────────────────────────────────────────

    /// Queues `amount` of the asset for conversion to shares when the
    /// current epoch is processed. Any ticket from an already-processed
    /// epoch is claimed first.
    pub fn deposit(env: Env, user: Address, amount: i128) -> Result<(), Error> {
        ttl::extend_instance(&env);
        user.require_auth();
        if amount < MIN_DEPOSIT {
            return Err(Error::BelowMinimumDeposit);
        }
        Self::claim_inner(&env, &user)?;
        token::Client::new(&env, &asset(&env)?).transfer(
            &user,
            &env.current_contract_address(),
            &amount,
        );
        let epoch = epoch(&env);
        let ticket = add_ticket(&env, &DataKey::Deposit(user.clone()), epoch, amount)?;
        let pending = add_i128(&env, &DataKey::PendingDeposits, amount)?;
        events::deposit_queued(
            &env,
            user,
            epoch,
            DepositQueuedEvent {
                amount,
                ticket,
                pending_deposits: pending,
            },
        );
        Ok(())
    }

    /// Queues `shares` for redemption at the current epoch's closing
    /// price. The shares leave the user's balance now, so they can't be
    /// requested twice. A request made mid-epoch simply waits for the
    /// epoch to close.
    pub fn request_withdraw(env: Env, user: Address, shares: i128) -> Result<(), Error> {
        ttl::extend_instance(&env);
        user.require_auth();
        if shares <= 0 {
            return Err(Error::InvalidAmount);
        }
        Self::claim_inner(&env, &user)?;
        let key = DataKey::Shares(user.clone());
        let balance = get_i128(&env, &key);
        if balance < shares {
            return Err(Error::InsufficientShares);
        }
        set_i128(&env, &key, balance - shares);
        let epoch = epoch(&env);
        let ticket = add_ticket(&env, &DataKey::Withdrawal(user.clone()), epoch, shares)?;
        let pending = add_i128(&env, &DataKey::PendingWithdrawShares, shares)?;
        events::withdraw_queued(
            &env,
            user,
            epoch,
            WithdrawQueuedEvent {
                shares,
                ticket,
                share_balance: balance - shares,
                pending_withdraw_shares: pending,
            },
        );
        Ok(())
    }

    /// Settles `user`'s tickets from processed epochs: credits deposit
    /// shares and pays out withdrawal assets. Callable by anyone, since
    /// proceeds only ever go to `user`. Returns `(shares, assets)`.
    pub fn claim(env: Env, user: Address) -> Result<(i128, i128), Error> {
        ttl::extend_instance(&env);
        Self::claim_inner(&env, &user)
    }

    // ── Keeper ────────────────────────────────────────────────────────────────

    /// Writes `contracts` of `series_id` with the pool as writer, locking
    /// exactly `collateral` (which must equal the market's required
    /// collateral — the pool authorizes a transfer of precisely that
    /// amount). Enforces the series whitelist/cap, the utilization cap,
    /// and that only idle assets are used. Returns the position id.
    pub fn write_option(
        env: Env,
        series_id: u64,
        contracts: i128,
        collateral: i128,
        min_premium: i128,
    ) -> Result<u64, Error> {
        ttl::extend_instance(&env);
        require_keeper(&env)?;
        if contracts <= 0 || collateral <= 0 {
            return Err(Error::InvalidAmount);
        }
        let cap: i128 = ttl::get_persistent(&env, &DataKey::SeriesCap(series_id)).unwrap_or(0);
        if cap == 0 {
            return Err(Error::SeriesNotWhitelisted);
        }
        let series_locked = get_i128(&env, &DataKey::SeriesLocked(series_id));
        if checked_add(series_locked, collateral)? > cap {
            return Err(Error::SeriesCapExceeded);
        }
        let idle = get_i128(&env, &DataKey::Idle);
        let locked = get_i128(&env, &DataKey::Locked);
        if collateral > idle {
            return Err(Error::InsufficientIdle);
        }
        let max_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxUtilizationBps)
            .ok_or(Error::NotInitialized)?;
        let new_locked = checked_add(locked, collateral)?;
        if mul(new_locked, BPS)? > mul(checked_add(idle, locked)?, max_bps.into())? {
            return Err(Error::UtilizationCapExceeded);
        }
        let mut positions = positions(&env);
        if positions.len() >= MAX_POSITIONS {
            return Err(Error::TooManyPositions);
        }

        let asset = asset(&env)?;
        let market = market(&env)?;
        let pool = env.current_contract_address();
        let usdc = token::Client::new(&env, &asset);
        let before = usdc.balance(&pool);
        // The market pulls collateral with `transfer(pool, market, amount)`
        // from inside its own frame, so the pool pre-authorizes exactly
        // that sub-call. This covers only the very next contract call.
        env.authorize_as_current_contract(vec![
            &env,
            InvokerContractAuthEntry::Contract(SubContractInvocation {
                context: ContractContext {
                    contract: asset.clone(),
                    fn_name: Symbol::new(&env, "transfer"),
                    args: (pool.clone(), market.clone(), collateral).into_val(&env),
                },
                sub_invocations: vec![&env],
            }),
        ]);
        let position_id: u64 = env.invoke_contract(
            &market,
            &Symbol::new(&env, "write_option"),
            vec![
                &env,
                pool.clone().into_val(&env),
                series_id.into_val(&env),
                contracts.into_val(&env),
                collateral.into_val(&env),
                min_premium.into_val(&env),
            ],
        );
        let after = usdc.balance(&pool);
        // The authorization above pins the outflow to `collateral`, so
        // whatever else changed is premium.
        let premium = delta(after, delta(before, collateral)?)?;

        let new_idle = checked_add(idle - collateral, premium)?;
        set_i128(&env, &DataKey::Idle, new_idle);
        set_i128(&env, &DataKey::Locked, new_locked);
        set_i128(
            &env,
            &DataKey::SeriesLocked(series_id),
            series_locked + collateral,
        );
        positions.push_back(PoolPosition {
            position_id,
            series_id,
            collateral,
        });
        ttl::set_persistent(&env, &DataKey::Positions, &positions);
        events::option_written(
            &env,
            series_id,
            position_id,
            OptionWrittenEvent {
                collateral,
                premium,
                idle: new_idle,
                locked: new_locked,
            },
        );
        Ok(position_id)
    }

    /// Reclaims a settled position's remaining collateral from the
    /// market. Whatever comes back goes to `idle`; any shortfall against
    /// the locked collateral is the pool's realized loss. Callable by
    /// anyone, since proceeds only ever go to the pool. Returns the
    /// amount returned.
    pub fn reclaim(env: Env, position_id: u64) -> Result<i128, Error> {
        ttl::extend_instance(&env);
        let mut positions = positions(&env);
        let index = positions
            .iter()
            .position(|p| p.position_id == position_id)
            .ok_or(Error::PositionNotFound)? as u32;
        let position = positions.get(index).ok_or(Error::PositionNotFound)?;

        let pool = env.current_contract_address();
        let usdc = token::Client::new(&env, &asset(&env)?);
        let before = usdc.balance(&pool);
        env.invoke_contract::<()>(
            &market(&env)?,
            &Symbol::new(&env, "reclaim_collateral"),
            vec![
                &env,
                pool.clone().into_val(&env),
                position_id.into_val(&env),
            ],
        );
        let returned = delta(usdc.balance(&pool), before)?;

        let idle = add_i128(&env, &DataKey::Idle, returned)?;
        let locked = add_i128(&env, &DataKey::Locked, -position.collateral)?;
        add_i128(
            &env,
            &DataKey::SeriesLocked(position.series_id),
            -position.collateral,
        )?;
        positions.remove(index);
        ttl::set_persistent(&env, &DataKey::Positions, &positions);
        if positions.is_empty() {
            env.storage()
                .instance()
                .set(&DataKey::ExpectedLiabilities, &0i128);
        }
        events::collateral_reclaimed(
            &env,
            position_id,
            CollateralReclaimedEvent {
                series_id: position.series_id,
                collateral: position.collateral,
                returned,
                idle,
                locked,
            },
        );
        Ok(returned)
    }

    /// Keeper's mark-to-market of what open positions are expected to pay
    /// out; only affects the informational `total_assets`/`share_price`.
    pub fn set_expected_liabilities(env: Env, amount: i128) -> Result<(), Error> {
        ttl::extend_instance(&env);
        require_keeper(&env)?;
        if amount < 0 {
            return Err(Error::InvalidAmount);
        }
        env.storage()
            .instance()
            .set(&DataKey::ExpectedLiabilities, &amount);
        events::liabilities_marked(
            &env,
            LiabilitiesMarkedEvent {
                expected_liabilities: amount,
            },
        );
        Ok(())
    }

    /// Closes the current epoch once every position has been reclaimed:
    /// fixes the share price from realized assets, converts all queued
    /// withdrawals and deposits at it, and opens the next epoch.
    pub fn process_epoch(env: Env) -> Result<EpochResult, Error> {
        ttl::extend_instance(&env);
        require_keeper(&env)?;
        if !positions(&env).is_empty() {
            return Err(Error::PositionsStillOpen);
        }
        let assets = get_i128(&env, &DataKey::Idle);
        let supply = get_i128(&env, &DataKey::TotalShares);
        let deposit_assets = get_i128(&env, &DataKey::PendingDeposits);
        let withdraw_shares = get_i128(&env, &DataKey::PendingWithdrawShares);

        // Both conversions round down, in the pool's favour.
        let withdraw_assets = to_assets(withdraw_shares, assets, supply)?;
        let deposit_shares = to_shares(deposit_assets, assets, supply)?;

        let idle = checked_add(assets - withdraw_assets, deposit_assets)?;
        set_i128(&env, &DataKey::Idle, idle);
        add_i128(&env, &DataKey::Claimable, withdraw_assets)?;
        let shares_after = checked_add(supply - withdraw_shares, deposit_shares)?;
        set_i128(&env, &DataKey::TotalShares, shares_after);
        set_i128(&env, &DataKey::PendingDeposits, 0);
        set_i128(&env, &DataKey::PendingWithdrawShares, 0);

        let epoch = epoch(&env);
        let result = EpochResult {
            deposit_assets,
            deposit_shares,
            withdraw_shares,
            withdraw_assets,
        };
        ttl::set_persistent(&env, &DataKey::EpochResult(epoch), &result);
        let next = epoch.checked_add(1).ok_or(Error::MathOverflow)?;
        env.storage().instance().set(&DataKey::Epoch, &next);
        events::epoch_processed(
            &env,
            epoch,
            EpochProcessedEvent {
                total_assets: assets,
                total_shares: supply,
                deposit_assets,
                deposit_shares,
                withdraw_shares,
                withdraw_assets,
                idle,
                total_shares_after: shares_after,
            },
        );
        Ok(result)
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn get_epoch(env: Env) -> Result<u32, Error> {
        Ok(epoch(&env))
    }

    /// `idle + locked − expected_liabilities`, floored at zero.
    pub fn total_assets(env: Env) -> Result<i128, Error> {
        total_assets(&env)
    }

    pub fn total_shares(env: Env) -> Result<i128, Error> {
        Ok(get_i128(&env, &DataKey::TotalShares))
    }

    /// Assets per share, scaled by `PRICE_SCALE`, including the virtual
    /// offset. Informational mid-epoch; settlement uses `process_epoch`.
    pub fn share_price(env: Env) -> Result<i128, Error> {
        let assets = checked_add(total_assets(&env)?, VIRTUAL_ASSETS)?;
        let shares = checked_add(get_i128(&env, &DataKey::TotalShares), VIRTUAL_SHARES)?;
        Ok(mul(assets, PRICE_SCALE)? / shares)
    }

    pub fn shares_of(env: Env, user: Address) -> Result<i128, Error> {
        Ok(get_i128(&env, &DataKey::Shares(user)))
    }

    pub fn idle(env: Env) -> Result<i128, Error> {
        Ok(get_i128(&env, &DataKey::Idle))
    }

    pub fn locked(env: Env) -> Result<i128, Error> {
        Ok(get_i128(&env, &DataKey::Locked))
    }

    pub fn get_deposit_ticket(env: Env, user: Address) -> Result<Option<Ticket>, Error> {
        Ok(ttl::get_persistent(&env, &DataKey::Deposit(user)))
    }

    pub fn get_withdrawal_ticket(env: Env, user: Address) -> Result<Option<Ticket>, Error> {
        Ok(ttl::get_persistent(&env, &DataKey::Withdrawal(user)))
    }

    pub fn get_epoch_result(env: Env, epoch: u32) -> Result<Option<EpochResult>, Error> {
        Ok(ttl::get_persistent(&env, &DataKey::EpochResult(epoch)))
    }

    pub fn get_positions(env: Env) -> Result<Vec<PoolPosition>, Error> {
        Ok(positions(&env))
    }
}

impl LpPool {
    fn claim_inner(env: &Env, user: &Address) -> Result<(i128, i128), Error> {
        let current = epoch(env);
        let mut shares = 0;
        let mut assets = 0;

        let deposit_key = DataKey::Deposit(user.clone());
        if let Some(t) = ticket(env, &deposit_key) {
            if t.epoch < current {
                let r = epoch_result(env, t.epoch)?;
                // t.amount <= r.deposit_assets, which is positive.
                shares = mul(t.amount, r.deposit_shares)? / r.deposit_assets;
                add_i128(env, &DataKey::Shares(user.clone()), shares)?;
                env.storage().persistent().remove(&deposit_key);
            }
        }

        let withdraw_key = DataKey::Withdrawal(user.clone());
        if let Some(t) = ticket(env, &withdraw_key) {
            if t.epoch < current {
                let r = epoch_result(env, t.epoch)?;
                assets = mul(t.amount, r.withdraw_assets)? / r.withdraw_shares;
                env.storage().persistent().remove(&withdraw_key);
                add_i128(env, &DataKey::Claimable, -assets)?;
                token::Client::new(env, &asset(env)?).transfer(
                    &env.current_contract_address(),
                    user,
                    &assets,
                );
            }
        }

        if shares > 0 || assets > 0 {
            events::claimed(
                env,
                user.clone(),
                ClaimedEvent {
                    shares,
                    assets,
                    share_balance: get_i128(env, &DataKey::Shares(user.clone())),
                },
            );
        }
        Ok((shares, assets))
    }
}

fn total_assets(env: &Env) -> Result<i128, Error> {
    let gross = checked_add(
        get_i128(env, &DataKey::Idle),
        get_i128(env, &DataKey::Locked),
    )?;
    Ok(delta(gross, get_i128(env, &DataKey::ExpectedLiabilities))?.max(0))
}

fn to_shares(assets_in: i128, assets: i128, supply: i128) -> Result<i128, Error> {
    Ok(
        mul(assets_in, checked_add(supply, VIRTUAL_SHARES)?)?
            / checked_add(assets, VIRTUAL_ASSETS)?,
    )
}

fn to_assets(shares_in: i128, assets: i128, supply: i128) -> Result<i128, Error> {
    Ok(
        mul(shares_in, checked_add(assets, VIRTUAL_ASSETS)?)?
            / checked_add(supply, VIRTUAL_SHARES)?,
    )
}

fn mul(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_mul(b).ok_or(Error::InvalidAmount)
}

fn checked_add(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_add(b).ok_or(Error::InvalidAmount)
}

/// `a - b` for token balance deltas.
fn delta(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_sub(b).ok_or(Error::MathOverflow)
}

fn check_bps(bps: u32) -> Result<(), Error> {
    if bps == 0 || i128::from(bps) > BPS {
        return Err(Error::InvalidUtilization);
    }
    Ok(())
}

fn instance_address(env: &Env, key: &DataKey) -> Result<Address, Error> {
    env.storage()
        .instance()
        .get(key)
        .ok_or(Error::NotInitialized)
}

fn require_admin(env: &Env) -> Result<(), Error> {
    ttl::extend_instance(env);
    instance_address(env, &DataKey::Admin)?.require_auth();
    Ok(())
}

fn require_keeper(env: &Env) -> Result<(), Error> {
    instance_address(env, &DataKey::Keeper)?.require_auth();
    Ok(())
}

fn asset(env: &Env) -> Result<Address, Error> {
    instance_address(env, &DataKey::Asset)
}

fn market(env: &Env) -> Result<Address, Error> {
    instance_address(env, &DataKey::Market)
}

fn epoch(env: &Env) -> u32 {
    env.storage().instance().get(&DataKey::Epoch).unwrap_or(0)
}

fn positions(env: &Env) -> Vec<PoolPosition> {
    ttl::get_persistent(env, &DataKey::Positions).unwrap_or_else(|| Vec::new(env))
}

fn ticket(env: &Env, key: &DataKey) -> Option<Ticket> {
    ttl::get_persistent(env, key)
}

fn epoch_result(env: &Env, epoch: u32) -> Result<EpochResult, Error> {
    ttl::get_persistent(env, &DataKey::EpochResult(epoch)).ok_or(Error::EpochNotProcessed)
}

/// Adds to a queued ticket for `epoch` and returns the ticket's total.
/// Callers claim any older ticket first, so an existing one is always
/// from this same epoch.
fn add_ticket(env: &Env, key: &DataKey, epoch: u32, amount: i128) -> Result<i128, Error> {
    let total = checked_add(ticket(env, key).map(|t| t.amount).unwrap_or(0), amount)?;
    ttl::set_persistent(
        env,
        key,
        &Ticket {
            epoch,
            amount: total,
        },
    );
    Ok(total)
}

/// Pool-wide totals live in instance storage, per-user and per-series
/// balances in persistent storage.
fn is_instance(key: &DataKey) -> bool {
    !matches!(key, DataKey::Shares(_) | DataKey::SeriesLocked(_))
}

fn get_i128(env: &Env, key: &DataKey) -> i128 {
    if is_instance(key) {
        env.storage().instance().get(key).unwrap_or(0)
    } else {
        ttl::get_persistent(env, key).unwrap_or(0)
    }
}

fn set_i128(env: &Env, key: &DataKey, value: i128) {
    if is_instance(key) {
        env.storage().instance().set(key, &value);
    } else {
        ttl::set_persistent(env, key, &value);
    }
}

/// Adds `delta` to `key` and returns the new value.
fn add_i128(env: &Env, key: &DataKey, delta: i128) -> Result<i128, Error> {
    let value = checked_add(get_i128(env, key), delta)?;
    set_i128(env, key, value);
    Ok(value)
}
