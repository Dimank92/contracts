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
pub use types::{Position, RewardConfig};
use types::{DataKey, PendingUnstake};

/// Scale of RewardPerToken. 1e18 keeps a 1-base-unit reward over a
/// 1e15-unit total stake (1e8 tokens at 7 decimals) representable, while
/// `amount × ACC_PRECISION` stays below i128::MAX for any amount up to
/// ~1.7e20 base units.
pub const ACC_PRECISION: i128 = 1_000_000_000_000_000_000;

/// Caps per-user update cost, which iterates every reward token.
pub const MAX_REWARD_TOKENS: u32 = 10;

/// Hard maximum page size for `list_positions`.
pub const MAX_PAGE_SIZE: u32 = 50;

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

/// Total `user` has accrued in `token` but not yet claimed, evaluated at
/// the current accumulator: the already-settled `Owed` balance plus the
/// unsettled delta since `UserPaid`. Pure read; shared by `earned`,
/// `get_accrued_rewards`, and `claim_rewards` so the query and the
/// mutation cannot diverge.
fn accrued(env: &Env, user: &Address, token: &Address) -> i128 {
    let stake = get_i128(env, &DataKey::Stake(user.clone()));
    let rpt = get_i128(env, &DataKey::RewardPerToken(token.clone()));
    let paid = get_i128(env, &DataKey::UserPaid(user.clone(), token.clone()));
    let pending = if stake > 0 && rpt > paid {
        stake.checked_mul(rpt - paid).unwrap() / ACC_PRECISION
    } else {
        0
    };
    get_i128(env, &DataKey::Owed(user.clone(), token.clone())) + pending
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

/// Records `user` in the staker index the first time they stake, so
/// `list_positions` can enumerate without replaying events.
fn index_staker(env: &Env, user: &Address) {
    let mut stakers: Vec<Address> = env
        .storage()
        .persistent()
        .get(&DataKey::Stakers)
        .unwrap_or(Vec::new(env));
    if !stakers.contains(user) {
        stakers.push_back(user.clone());
        env.storage().persistent().set(&DataKey::Stakers, &stakers);
        let count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::StakerCount)
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&DataKey::StakerCount, &(count + 1));
    }
}

/// Reads a persistent key, extending its TTL per the workspace policy.
fn get_persistent<V: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>>(
    env: &Env,
    key: &DataKey,
) -> Option<V> {
    let storage = env.storage().persistent();
    let value = storage.get(key);
    if value.is_some() {
        storage.extend_ttl(key, zenith_common::ttl::THRESHOLD, zenith_common::ttl::EXTEND);
    }
    value
}

/// Builds a `Position` for `user` from stored principal and pending
/// unstake state. Unknown stakers get an all-zero position.
fn position_of(env: &Env, user: &Address) -> Position {
    let staked = get_i128(env, &DataKey::Stake(user.clone()));
    let pending: Option<PendingUnstake> = get_persistent(env, &DataKey::PendingUnstake(user.clone()));
    match pending {
        Some(p) => Position {
            staked,
            pending_unstake: p.amount,
            unlock_at: p.unlock_at,
        },
        None => Position {
            staked,
            pending_unstake: 0,
            unlock_at: 0,
        },
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
        index_staker(&env, &user);
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
        for token in reward_tokens(&env).iter() {
            let owed_key = DataKey::Owed(user.clone(), token.clone());
            let owed = accrued(&env, &user, &token);
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
        let splitter: Address = env.storage().instance().get(&DataKey::Splitter).unwrap();
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

    pub fn schema_version(env: Env) -> u32 {
        zenith_common::migrations::schema_version(&env)
    }

    /// `(target, cursor)` while a paginated migration is in progress.
    pub fn migration_state(env: Env) -> Option<(u32, u32)> {
        zenith_common::migrations::migration_state(&env)
    }

    // ── Views ────────────────────────────────────────────────────────────────

    /// Rewards `user` has accrued in `token` but not yet claimed, evaluated
    /// at the current accumulator. Read-only.
    pub fn earned(env: Env, user: Address, token: Address) -> i128 {
        accrued(&env, &user, &token)
    }

    /// `user`'s currently earning stake. Zero for unknown stakers.
    pub fn staked_of(env: Env, user: Address) -> i128 {
        get_i128(&env, &DataKey::Stake(user))
    }

    /// Sum of every staker's earning stake.
    pub fn total_staked(env: Env) -> i128 {
        total_staked(&env)
    }

    /// `user`'s pending unstake, if any.
    pub fn pending_unstake(env: Env, user: Address) -> Option<PendingUnstake> {
        get_persistent(&env, &DataKey::PendingUnstake(user))
    }

    /// Every reward token currently registered.
    pub fn reward_tokens(env: Env) -> Vec<Address> {
        reward_tokens(&env)
    }

    /// Rewards carried over because nothing was staked when they arrived.
    pub fn undistributed(env: Env, token: Address) -> i128 {
        get_i128(&env, &DataKey::Undistributed(token))
    }

    /// The pinned fee splitter allowed to call `notify_reward`.
    pub fn get_splitter(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Splitter).unwrap()
    }

    /// Unstake cooldown in seconds.
    pub fn get_cooldown(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::Cooldown).unwrap()
    }

    /// `user`'s full position: earning stake plus any pending unstake and
    /// its unlock time. Unknown stakers get an all-zero position rather
    /// than an error. Read-only.
    pub fn get_position(env: Env, user: Address) -> Position {
        position_of(&env, &user)
    }

    /// Every reward token with the amount `user` has accrued but not yet
    /// claimed, computed at the current ledger timestamp without mutating
    /// state. Tokens with zero accrual are included with a zero amount.
    pub fn get_accrued_rewards(env: Env, user: Address) -> Vec<(Address, i128)> {
        let mut out = Vec::new(&env);
        for token in reward_tokens(&env).iter() {
            let amount = accrued(&env, &user, &token);
            out.push_back((token, amount));
        }
        out
    }

    /// Sum of every staker's earning stake. Alias of `total_staked` for
    /// the query surface.
    pub fn get_total_staked(env: Env) -> i128 {
        total_staked(&env)
    }

    /// The contract's current reward configuration: stake token, splitter,
    /// cooldown, and registered reward tokens. Read-only.
    pub fn get_reward_config(env: Env) -> RewardConfig {
        RewardConfig {
            stake_token: env.storage().instance().get(&DataKey::StakeToken).unwrap(),
            splitter: env.storage().instance().get(&DataKey::Splitter).unwrap(),
            cooldown: env.storage().instance().get(&DataKey::Cooldown).unwrap(),
            reward_tokens: reward_tokens(&env),
        }
    }

    /// Bounded enumeration of staker positions, in first-stake order.
    /// `limit` must be in `1 ..= MAX_PAGE_SIZE`; an offset past the end
    /// yields an empty vector. Read-only.
    pub fn list_positions(env: Env, offset: u32, limit: u32) -> Vec<(Address, Position)> {
        if limit == 0 {
            panic_with_error!(&env, Error::InvalidPageLimit);
        }
        if limit > MAX_PAGE_SIZE {
            panic_with_error!(&env, Error::PageLimitTooLarge);
        }
        let mut out = Vec::new(&env);
        let stakers: Vec<Address> = match get_persistent(&env, &DataKey::Stakers) {
            Some(s) => s,
            None => return out,
        };
        let len = stakers.len();
        if offset >= len {
            return out;
        }
        let end = core::cmp::min(offset.saturating_add(limit), len);
        let mut i = offset;
        while i < end {
            let user = stakers.get_unchecked(i);
            out.push_back((user.clone(), position_of(&env, &user)));
            i += 1;
        }
        out
    }
}
