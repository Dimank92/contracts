// Public API docs are enforced by `cargo xtask doc` (#268): the gate builds
// rustdoc, where this lint is on, and only there — soroban-sdk macro output is
// undocumented, which would otherwise fail `clippy -D warnings`.
#![cfg_attr(doc, warn(missing_docs))]
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

/// Staking contract: stake a token and accrue rewards per share.
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
                let accrued = stake
                    .checked_mul(rpt - paid)
                    .unwrap_or_else(|| panic_with_error!(env, Error::InternalInvariant))
                    / ACC_PRECISION;
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
        let delta = pending
            .checked_mul(ACC_PRECISION)
            .unwrap_or_else(|| panic_with_error!(env, Error::InternalInvariant))
            / total;
        set_i128(env, &rpt_key, get_i128(env, &rpt_key) + delta);
        set_i128(env, &carry_key, 0);
    }
}

#[contractimpl]
impl Staking {
    /// Sets the stake token, reward splitter and unstake cooldown. Fails with `AlreadyInitialized` on a second call.
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

    /// Stakes `amount` of the stake token from `user`. Requires `user` auth; fails with `InvalidAmount` if `amount <= 0`.
    pub fn stake(env: Env, user: Address, amount: i128) {
        user.require_auth();
        migrations::require_current(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        update_user(&env, &user);

        let stake_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::StakeToken)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
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

        let cooldown: u64 = env
            .storage()
            .instance()
            .get(&DataKey::Cooldown)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
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

        let stake_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::StakeToken)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
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
                events::reward_claimed(&env, user.clone(), token, owed);
            }
        }
    }

    /// Callable only by the pinned splitter, which must have `amount` of
    /// `token` to transfer in. Any SEP-41 token can be a reward token, up
    /// to MAX_REWARD_TOKENS distinct ones.
    pub fn notify_reward(env: Env, token: Address, amount: i128) {
        let splitter: Address = env
            .storage()
            .instance()
            .get(&DataKey::Splitter)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant));
        splitter.require_auth();
        migrations::require_current(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let mut tokens = reward_tokens(&env);
        if !tokens.contains(&token) {
            if tokens.len() >= MAX_REWARD_TOKENS {
                panic_with_error!(&env, Error::TooManyRewardTokens);
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
        distribute(&env, &token, amount);
        events::reward_notified(&env, token, amount);
    }

    // ── Schema migrations (docs/migrations.md) ──────────────────────────────

    /// Runs the registered steps `from+1 ..= to`. Returns the version
    /// reached (less than `to` if a step paginated). Permissionless:
    /// staking has no admin, and a new step can only arrive with a new
    /// wasm (see docs/migrations.md, "Governance decisions").
    pub fn migrate(env: Env, from: u32, to: u32) -> u32 {
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
    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> u32 {
        zenith_common::migrations::migrate_batch(
            &env,
            cursor,
            limit,
            migrations::step,
            &migrations::ERRORS,
        )
    }

    /// Stored schema version. Read-only.
    pub fn schema_version(env: Env) -> u32 {
        zenith_common::migrations::schema_version(&env)
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        zenith_common::migrations::migration_state(&env)
    }

    // ── Views ────────────────────────────────────────────────────────────────

    /// Rewards in `token` accrued to `user` and not yet claimed. Read-only.
    pub fn earned(env: Env, user: Address, token: Address) -> i128 {
        let stake = get_i128(&env, &DataKey::Stake(user.clone()));
        let rpt = get_i128(&env, &DataKey::RewardPerToken(token.clone()));
        let paid = get_i128(&env, &DataKey::UserPaid(user.clone(), token.clone()));
        get_i128(&env, &DataKey::Owed(user, token))
            + stake
                .checked_mul(rpt - paid)
                .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant))
                / ACC_PRECISION
    }

    /// `user`'s staked balance. Read-only.
    pub fn staked_of(env: Env, user: Address) -> i128 {
        get_i128(&env, &DataKey::Stake(user))
    }

    /// Total staked balance. Read-only.
    pub fn total_staked(env: Env) -> i128 {
        total_staked(&env)
    }

    /// `user`'s pending unstake request, if any. Read-only.
    pub fn pending_unstake(env: Env, user: Address) -> Option<PendingUnstake> {
        env.storage()
            .persistent()
            .get(&DataKey::PendingUnstake(user))
    }

    /// Tokens that have been distributed as rewards. Read-only.
    pub fn reward_tokens(env: Env) -> Vec<Address> {
        reward_tokens(&env)
    }

    /// Reward amount of `token` held while nobody was staked. Read-only.
    pub fn undistributed(env: Env, token: Address) -> i128 {
        get_i128(&env, &DataKey::Undistributed(token))
    }

    /// Reward splitter address. Read-only.
    pub fn get_splitter(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Splitter)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant))
    }

    /// Unstake cooldown in seconds. Read-only.
    pub fn get_cooldown(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::Cooldown)
            .unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant))
    }
}
