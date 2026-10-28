#![no_std]

//! Zenith Staking — stake ZEN, earn a pro-rata share of protocol fees.
//!
//! Synthetix `StakingRewards`-style accumulator adapted for instantaneous
//! notifications: every `notify_reward(token, amount)` from the pinned fee
//! splitter bumps `RewardPerToken(token)` by `amount × ACC_PRECISION /
//! total_staked`, and each staker's accrued rewards are
//! `stake × (RewardPerToken − UserPaid) / ACC_PRECISION`. All updates are
//! O(1) per reward token, independent of the number of stakers.
//!
//! Both divisions floor, so the sum of every staker's accrual can never
//! exceed what was notified; the rounding dust (strictly less than
//! `total_staked / ACC_PRECISION` base units per notification) stays in the
//! contract. See docs/staking-economics.md.
//!
//! Unstaking is two-step: `request_unstake` stops the amount earning
//! immediately and starts the cooldown; `withdraw` returns it afterward.

use soroban_sdk::{contract, contractimpl, token, Address, Env, Vec};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_events;

mod error;
mod events;
mod migrations;
mod types;

pub use error::Error;
pub use events::{
    CarryoverDistributedEvent, InitializedEvent, RewardClaimedEvent, RewardNotifiedEvent,
    StakedEvent, UnstakeRequestedEvent, WithdrawnEvent,
};
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::DataKey;
pub use types::PendingUnstake;

zenith_common::contract_meta!();

/// Scale of RewardPerToken. 1e18 keeps a 1-base-unit reward over a
/// 1e15-unit total stake (1e8 tokens at 7 decimals) representable, while
/// `amount × ACC_PRECISION` stays below i128::MAX for any amount up to
/// ~1.7e20 base units.
pub const ACC_PRECISION: i128 = 1_000_000_000_000_000_000;

/// Caps per-user update cost, which iterates every reward token.
pub const MAX_REWARD_TOKENS: u32 = 10;

#[contract]
pub struct Staking;

fn get_i128(env: &Env, key: &DataKey) -> i128 {
    env.storage().persistent().get(key).unwrap_or(0)
}

fn set_i128(env: &Env, key: &DataKey, value: i128) {
    env.storage().persistent().set(key, &value);
}

fn add(a: i128, b: i128) -> Result<i128, Error> {
    a.checked_add(b).ok_or(Error::MathOverflow)
}

fn instance<V: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>>(
    env: &Env,
    key: &DataKey,
) -> Result<V, Error> {
    env.storage()
        .instance()
        .get(key)
        .ok_or(Error::NotInitialized)
}

fn total_staked(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalStaked)
        .unwrap_or(0)
}

fn reward_tokens(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::RewardTokens)
        .unwrap_or(Vec::new(env))
}

/// `stake × (rpt − paid) / ACC_PRECISION`, floored.
fn accrued(stake: i128, rpt: i128, paid: i128) -> Result<i128, Error> {
    Ok(stake.checked_mul(rpt - paid).ok_or(Error::MathOverflow)? / ACC_PRECISION)
}

/// Settles `user`'s accrual in every reward token up to the current
/// accumulator. Must run before any change to the user's stake.
fn update_user(env: &Env, user: &Address) -> Result<(), Error> {
    let stake = get_i128(env, &DataKey::Stake(user.clone()));
    for token in reward_tokens(env).iter() {
        let rpt = get_i128(env, &DataKey::RewardPerToken(token.clone()));
        let paid_key = DataKey::UserPaid(user.clone(), token.clone());
        let paid = get_i128(env, &paid_key);
        if rpt != paid {
            if stake > 0 {
                let owed_key = DataKey::Owed(user.clone(), token.clone());
                let owed = add(get_i128(env, &owed_key), accrued(stake, rpt, paid)?)?;
                set_i128(env, &owed_key, owed);
            }
            set_i128(env, &paid_key, rpt);
        }
    }
    Ok(())
}

/// Adds `amount` plus any carried-over rewards for `token` to the
/// accumulator, or carries it all over if nothing is staked. Returns
/// `(amount distributed, new RewardPerToken, still carried over)`.
fn distribute(env: &Env, token: &Address, amount: i128) -> Result<(i128, i128, i128), Error> {
    let carry_key = DataKey::Undistributed(token.clone());
    let pending = add(amount, get_i128(env, &carry_key))?;
    let total = total_staked(env);
    let rpt_key = DataKey::RewardPerToken(token.clone());
    let rpt = get_i128(env, &rpt_key);
    if total == 0 {
        set_i128(env, &carry_key, pending);
        return Ok((0, rpt, pending));
    }
    if pending > 0 {
        let delta = pending
            .checked_mul(ACC_PRECISION)
            .ok_or(Error::MathOverflow)?
            / total;
        let rpt = add(rpt, delta)?;
        set_i128(env, &rpt_key, rpt);
        set_i128(env, &carry_key, 0);
        return Ok((pending, rpt, 0));
    }
    Ok((0, rpt, 0))
}

#[contractimpl]
impl Staking {
    pub fn initialize(
        env: Env,
        stake_token: Address,
        splitter: Address,
        cooldown: u64,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::StakeToken) {
            return Err(Error::AlreadyInitialized);
        }
        env.storage()
            .instance()
            .set(&DataKey::StakeToken, &stake_token);
        env.storage().instance().set(&DataKey::Splitter, &splitter);
        env.storage().instance().set(&DataKey::Admin, &splitter);
        env.storage().instance().set(&DataKey::Cooldown, &cooldown);
        zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION);
        events::initialized(
            &env,
            InitializedEvent {
                stake_token,
                splitter,
                cooldown,
                schema_version: CURRENT_SCHEMA_VERSION,
            },
        );
        Ok(())
    }

    pub fn stake(env: Env, user: Address, amount: i128) -> Result<(), Error> {
        user.require_auth();
        migrations::require_current(&env)?;
        if Self::emergency_unstake_enabled(env.clone())? {
            return Err(Error::EmergencyModeActive);
        }
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        update_user(&env, &user)?;

        let stake_token: Address = instance(&env, &DataKey::StakeToken)?;
        token::Client::new(&env, &stake_token).transfer(
            &user,
            &env.current_contract_address(),
            &amount,
        );

        let was_empty = total_staked(&env) == 0;
        let key = DataKey::Stake(user.clone());
        let stake = add(get_i128(&env, &key), amount)?;
        set_i128(&env, &key, stake);
        let total = add(total_staked(&env), amount)?;
        env.storage().instance().set(&DataKey::TotalStaked, &total);

        // Rewards that arrived while nothing was staked go to whoever
        // stakes first, rather than sitting idle until the next notify.
        if was_empty {
            for token in reward_tokens(&env).iter() {
                let (amount, reward_per_token, _) = distribute(&env, &token, 0)?;
                if amount > 0 {
                    events::carryover_distributed(
                        &env,
                        token,
                        CarryoverDistributedEvent {
                            amount,
                            reward_per_token,
                            total_staked: total,
                        },
                    );
                }
            }
        }
        events::staked(
            &env,
            user,
            StakedEvent {
                amount,
                stake,
                total_staked: total,
            },
        );
        Ok(())
    }

    /// Moves `amount` out of the earning stake into a pending unstake that
    /// becomes withdrawable after the cooldown. A new request adds to any
    /// existing pending amount and restarts the cooldown.
    pub fn request_unstake(env: Env, user: Address, amount: i128) -> Result<(), Error> {
        user.require_auth();
        migrations::require_current(&env)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let key = DataKey::Stake(user.clone());
        let stake = get_i128(&env, &key);
        if amount > stake {
            return Err(Error::InsufficientStake);
        }
        update_user(&env, &user)?;
        set_i128(&env, &key, stake - amount);
        // amount <= stake <= total_staked, so neither subtraction underflows.
        let total = total_staked(&env) - amount;
        env.storage().instance().set(&DataKey::TotalStaked, &total);

        let cooldown: u64 = instance(&env, &DataKey::Cooldown)?;
        let pending_key = DataKey::PendingUnstake(user.clone());
        let prev: Option<PendingUnstake> = env.storage().persistent().get(&pending_key);
        let pending = PendingUnstake {
            amount: add(prev.map(|p| p.amount).unwrap_or(0), amount)?,
            unlock_at: env
                .ledger()
                .timestamp()
                .checked_add(cooldown)
                .ok_or(Error::MathOverflow)?,
        };
        env.storage().persistent().set(&pending_key, &pending);
        events::unstake_requested(
            &env,
            user,
            UnstakeRequestedEvent {
                amount,
                stake: stake - amount,
                pending: pending.amount,
                unlock_at: pending.unlock_at,
                total_staked: total,
            },
        );
        Ok(())
    }

    /// Returns the pending unstaked amount once the cooldown has elapsed.
    pub fn withdraw(env: Env, user: Address) -> Result<i128, Error> {
        user.require_auth();
        migrations::require_current(&env)?;
        let pending_key = DataKey::PendingUnstake(user.clone());
        let pending: PendingUnstake = env
            .storage()
            .persistent()
            .get(&pending_key)
            .ok_or(Error::NothingToWithdraw)?;
        if env.ledger().timestamp() < pending.unlock_at {
            return Err(Error::CooldownActive);
        }
        env.storage().persistent().remove(&pending_key);

        let stake_token: Address = instance(&env, &DataKey::StakeToken)?;
        token::Client::new(&env, &stake_token).transfer(
            &env.current_contract_address(),
            &user,
            &pending.amount,
        );
        events::withdrawn(
            &env,
            user,
            WithdrawnEvent {
                amount: pending.amount,
            },
        );
        Ok(pending.amount)
    }

    /// Pays out everything `user` has accrued in every reward token.
    pub fn claim_rewards(env: Env, user: Address) -> Result<(), Error> {
        user.require_auth();
        migrations::require_current(&env)?;
        update_user(&env, &user)?;
        for token in reward_tokens(&env).iter() {
            let owed_key = DataKey::Owed(user.clone(), token.clone());
            let owed = get_i128(&env, &owed_key);
            if owed > 0 {
                set_i128(&env, &owed_key, 0);
                token::Client::new(&env, &token).transfer(
                    &env.current_contract_address(),
                    &user,
                    &owed,
                );
                events::reward_claimed(
                    &env,
                    user.clone(),
                    token,
                    RewardClaimedEvent { amount: owed },
                );
            }
        }
        Ok(())
    }

    /// Callable only by the pinned splitter, which must have `amount` of
    /// `token` to transfer in. Any SEP-41 token can be a reward token, up
    /// to MAX_REWARD_TOKENS distinct ones.
    pub fn notify_reward(env: Env, token: Address, amount: i128) -> Result<(), Error> {
        let splitter: Address = instance(&env, &DataKey::Splitter)?;
        splitter.require_auth();
        migrations::require_current(&env)?;
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let mut tokens = reward_tokens(&env);
        if !tokens.contains(&token) {
            if tokens.len() >= MAX_REWARD_TOKENS {
                return Err(Error::TooManyRewardTokens);
            }
            tokens.push_back(token.clone());
            env.storage()
                .instance()
                .set(&DataKey::RewardTokens, &tokens);
        }

        token::Client::new(&env, &token).transfer(
            &splitter,
            &env.current_contract_address(),
            &amount,
        );
        let (_, reward_per_token, undistributed) = distribute(&env, &token, amount)?;
        events::reward_notified(
            &env,
            token,
            RewardNotifiedEvent {
                amount,
                reward_per_token,
                undistributed,
                total_staked: total_staked(&env),
            },
        );
        Ok(())
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Returns the version
    /// reached (less than `to` if a step paginated). Permissionless:
    /// staking has no admin, and a new step can only arrive with a new
    /// wasm (see docs/migrations.md, "Governance decisions").
    pub fn migrate(env: Env, from: u32, to: u32) -> Result<u32, Error> {
        zenith_common::migrations::migrate(
            &env,
            from,
            to,
            CURRENT_SCHEMA_VERSION,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    /// Continues a paginated migration from `cursor`. Permissionless, like
    /// `bump`: the target was fixed by `migrate` and the steps are code in
    /// this wasm, so a keeper can only advance it.
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> Result<u32, Error> {
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    pub fn schema_version(env: Env) -> Result<u32, Error> {
        Ok(zenith_common::migrations::schema_version(&env))
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Result<Option<(u32, u32)>, Error> {
        Ok(zenith_common::migrations::migration_state(&env))
    }

    // ── Views ────────────────────────────────────────────────────────────────

    pub fn earned(env: Env, user: Address, token: Address) -> Result<i128, Error> {
        let stake = get_i128(&env, &DataKey::Stake(user.clone()));
        let rpt = get_i128(&env, &DataKey::RewardPerToken(token.clone()));
        let paid = get_i128(&env, &DataKey::UserPaid(user.clone(), token.clone()));
        add(
            get_i128(&env, &DataKey::Owed(user, token)),
            accrued(stake, rpt, paid)?,
        )
    }

    pub fn staked_of(env: Env, user: Address) -> Result<i128, Error> {
        Ok(get_i128(&env, &DataKey::Stake(user)))
    }

    pub fn total_staked(env: Env) -> Result<i128, Error> {
        Ok(total_staked(&env))
    }

    pub fn pending_unstake(env: Env, user: Address) -> Result<Option<PendingUnstake>, Error> {
        Ok(env
            .storage()
            .persistent()
            .get(&DataKey::PendingUnstake(user)))
    }

    pub fn reward_tokens(env: Env) -> Result<Vec<Address>, Error> {
        Ok(reward_tokens(&env))
    }

    pub fn undistributed(env: Env, token: Address) -> Result<i128, Error> {
        Ok(get_i128(&env, &DataKey::Undistributed(token)))
    }

    pub fn get_splitter(env: Env) -> Result<Address, Error> {
        instance(&env, &DataKey::Splitter)
    }

    pub fn get_cooldown(env: Env) -> Result<u64, Error> {
        instance(&env, &DataKey::Cooldown)
    }

    pub fn cooldown(env: Env) -> Result<u64, Error> {
        instance(&env, &DataKey::Cooldown)
    }

    pub fn set_reward_rate(env: Env, rate: u32) -> Result<(), Error> {
        let admin: Address = instance(&env, &DataKey::Admin)?;
        admin.require_auth();
        migrations::require_current(&env)?;
        if rate > 10_000 {
            return Err(Error::ValueOutOfRange);
        }
        env.storage().instance().set(&DataKey::RewardRate, &rate);
        env.events().publish((soroban_sdk::Symbol::new(&env, "reward_rate_set"),), rate);
        Ok(())
    }

    pub fn reward_rate(env: Env) -> Result<u32, Error> {
        Ok(env.storage().instance().get(&DataKey::RewardRate).unwrap_or(0))
    }

    pub fn set_cooldown(env: Env, cooldown: u64) -> Result<(), Error> {
        let admin: Address = instance(&env, &DataKey::Admin)?;
        admin.require_auth();
        migrations::require_current(&env)?;
        env.storage().instance().set(&DataKey::Cooldown, &cooldown);
        env.events().publish((soroban_sdk::Symbol::new(&env, "cooldown_set"),), cooldown);
        Ok(())
    }

    pub fn set_emergency_unstake(env: Env, enabled: bool) -> Result<(), Error> {
        let admin: Address = instance(&env, &DataKey::Admin)?;
        admin.require_auth();
        migrations::require_current(&env)?;
        env.storage().instance().set(&DataKey::EmergencyMode, &enabled);
        env.events().publish((soroban_sdk::Symbol::new(&env, "emergency_unstake_set"),), enabled);
        Ok(())
    }

    pub fn emergency_unstake_enabled(env: Env) -> Result<bool, Error> {
        Ok(env.storage().instance().get(&DataKey::EmergencyMode).unwrap_or(false))
    }

    pub fn emergency_unstake(env: Env, user: Address) -> Result<i128, Error> {
        user.require_auth();
        migrations::require_current(&env)?;
        if !Self::emergency_unstake_enabled(env.clone())? {
            return Err(Error::EmergencyModeDisabled);
        }
        let key = DataKey::Stake(user.clone());
        let stake = get_i128(&env, &key);
        if stake <= 0 {
            return Err(Error::InsufficientStake);
        }
        set_i128(&env, &key, 0);
        let total = total_staked(&env) - stake;
        env.storage().instance().set(&DataKey::TotalStaked, &total);

        let stake_token: Address = instance(&env, &DataKey::StakeToken)?;
        token::Client::new(&env, &stake_token).transfer(
            &env.current_contract_address(),
            &user,
            &stake,
        );
        events::withdrawn(
            &env,
            user,
            WithdrawnEvent {
                amount: stake,
            },
        );
        Ok(stake)
    }
}
