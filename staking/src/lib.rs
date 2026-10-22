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

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env, Vec};

#[cfg(test)]
mod test;

mod error;
mod events;
mod migrations;
mod types;

use error::Error;
pub use migrations::CURRENT_SCHEMA_VERSION;
use types::{DataKey, PendingUnstake};

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

/// Settles `user`'s accrual in every reward token up to the current
/// accumulator. Must run before any change to the user's stake.
fn update_user(env: &Env, user: &Address) {
    let stake = get_i128(env, &DataKey::Stake(user.clone()));
    for token in reward_tokens(env).iter() {
        let rpt = get_i128(env, &DataKey::RewardPerToken(token.clone()));
        let paid_key = DataKey::UserPaid(user.clone(), token.clone());
        let paid = get_i128(env, &paid_key);
        if rpt != paid {
            if stake > 0 {
                let owed_key = DataKey::Owed(user.clone(), token.clone());
                let accrued = stake.checked_mul(rpt - paid).unwrap() / ACC_PRECISION;
                set_i128(env, &owed_key, get_i128(env, &owed_key) + accrued);
            }
            set_i128(env, &paid_key, rpt);
        }
    }
}

/// Adds `amount` plus any carried-over rewards for `token` to the
/// accumulator, or carries it all over if nothing is staked.
fn distribute(env: &Env, token: &Address, amount: i128) {
    let carry_key = DataKey::Undistributed(token.clone());
    let pending = amount + get_i128(env, &carry_key);
    let total = total_staked(env);
    if total == 0 {
        set_i128(env, &carry_key, pending);
        return;
    }
    if pending > 0 {
        let rpt_key = DataKey::RewardPerToken(token.clone());
        let delta = pending.checked_mul(ACC_PRECISION).unwrap() / total;
        set_i128(env, &rpt_key, get_i128(env, &rpt_key) + delta);
        set_i128(env, &carry_key, 0);
    }
}

#[contractimpl]
impl Staking {
    pub fn initialize(env: Env, stake_token: Address, splitter: Address, cooldown: u64) {
        if env.storage().instance().has(&DataKey::StakeToken) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage()
            .instance()
            .set(&DataKey::StakeToken, &stake_token);
        env.storage().instance().set(&DataKey::Splitter, &splitter);
        env.storage().instance().set(&DataKey::Cooldown, &cooldown);
        zenith_common::migrations::init_schema(&env, CURRENT_SCHEMA_VERSION);
    }

    pub fn stake(env: Env, user: Address, amount: i128) {
        user.require_auth();
        migrations::require_current(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        update_user(&env, &user);

        let stake_token: Address = env.storage().instance().get(&DataKey::StakeToken).unwrap();
        token::Client::new(&env, &stake_token).transfer(
            &user,
            &env.current_contract_address(),
            &amount,
        );

        let was_empty = total_staked(&env) == 0;
        let key = DataKey::Stake(user.clone());
        set_i128(&env, &key, get_i128(&env, &key) + amount);
        env.storage()
            .instance()
            .set(&DataKey::TotalStaked, &(total_staked(&env) + amount));

        // Rewards that arrived while nothing was staked go to whoever
        // stakes first, rather than sitting idle until the next notify.
        if was_empty {
            for token in reward_tokens(&env).iter() {
                distribute(&env, &token, 0);
            }
        }
        events::staked(&env, user, amount);
    }

    /// Moves `amount` out of the earning stake into a pending unstake that
    /// becomes withdrawable after the cooldown. A new request adds to any
    /// existing pending amount and restarts the cooldown.
    pub fn request_unstake(env: Env, user: Address, amount: i128) {
        user.require_auth();
        migrations::require_current(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let key = DataKey::Stake(user.clone());
        let stake = get_i128(&env, &key);
        if amount > stake {
            panic_with_error!(&env, Error::InsufficientStake);
        }
        update_user(&env, &user);
        set_i128(&env, &key, stake - amount);
        env.storage()
            .instance()
            .set(&DataKey::TotalStaked, &(total_staked(&env) - amount));

        let cooldown: u64 = env.storage().instance().get(&DataKey::Cooldown).unwrap();
        let pending_key = DataKey::PendingUnstake(user.clone());
        let prev: Option<PendingUnstake> = env.storage().persistent().get(&pending_key);
        let pending = PendingUnstake {
            amount: prev.map(|p| p.amount).unwrap_or(0) + amount,
            unlock_at: env.ledger().timestamp() + cooldown,
        };
        env.storage().persistent().set(&pending_key, &pending);
        events::unstake_requested(&env, user, amount, pending.unlock_at);
    }

    /// Returns the pending unstaked amount once the cooldown has elapsed.
    pub fn withdraw(env: Env, user: Address) -> i128 {
        user.require_auth();
        migrations::require_current(&env);
        let pending_key = DataKey::PendingUnstake(user.clone());
        let pending: PendingUnstake = env
            .storage()
            .persistent()
            .get(&pending_key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NothingToWithdraw));
        if env.ledger().timestamp() < pending.unlock_at {
            panic_with_error!(&env, Error::CooldownActive);
        }
        env.storage().persistent().remove(&pending_key);

        let stake_token: Address = env.storage().instance().get(&DataKey::StakeToken).unwrap();
        token::Client::new(&env, &stake_token).transfer(
            &env.current_contract_address(),
            &user,
            &pending.amount,
        );
        events::withdrawn(&env, user, pending.amount);
        pending.amount
    }

    /// Pays out everything `user` has accrued in every reward token.
    pub fn claim_rewards(env: Env, user: Address) {
        user.require_auth();
        migrations::require_current(&env);
        update_user(&env, &user);
    }
}
