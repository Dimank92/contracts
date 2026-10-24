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
use types::{AdminConfig, DataKey, PendingUnstake};

/// Scale of RewardPerToken. 1e18 keeps a 1-base-unit reward over a
/// 1e15-unit total stake (1e8 tokens at 7 decimals) representable, while
/// `amount × ACC_PRECISION` stays below i128::MAX for any amount up to
/// ~1.7e20 base units.
pub const ACC_PRECISION: i128 = 1_000_000_000_000_000_000;

/// Caps per-user update cost, which iterates every reward token.
pub const MAX_REWARD_TOKENS: u32 = 10;

/// Maximum reward rate an admin may configure, in basis points of the
/// notified reward amount that is actually distributed to stakers.
/// 10_000 bps = 100% (the default). Rates above 100% are rejected so a
/// misconfiguration can never promise more than was notified.
pub const MAX_REWARD_RATE_BPS: u32 = 10_000;

/// Upper bound on the unbonding cooldown an admin may configure (30 days).
/// Guards against an accidental cooldown that would lock stakers out.
pub const MAX_COOLDOWN_SECS: u64 = 30 * 24 * 60 * 60;

/// TTL extension applied to every configuration write, per the workspace
/// storage policy (see docs/storage.md).
const CONFIG_TTL_THRESHOLD: u32 = 100_000;
const CONFIG_TTL_EXTEND: u32 = 500_000;

#[contract]
pub struct Staking;

fn get_i128(env: &Env, key: &DataKey) -> i128 {
    env.storage().persistent().get(key).unwrap_or(0)
}

fn set_i128(env: &Env, key: &DataKey, value: i128) {
    env.storage().persistent().set(key, &value);
}

/// Single accessor for the admin-tunable configuration. Both the accrual
/// path (`distribute`) and the admin path read through this so they can
/// never disagree. Missing fields fall back to the documented defaults:
/// 100% reward rate, the cooldown set at `initialize`, and no emergency.
fn config(env: &Env) -> AdminConfig {
    let default_cooldown: u64 = env
        .storage()
        .instance()
        .get(&DataKey::Cooldown)
        .unwrap_or(0);
    env.storage()
        .instance()
        .get(&DataKey::AdminConfig)
        .unwrap_or(AdminConfig {
            reward_rate_bps: MAX_REWARD_RATE_BPS,
            cooldown: default_cooldown,
            emergency_exit: false,
            stakes_blocked: false,
        })
}

/// Persists `cfg` and extends its TTL per the workspace storage policy.
fn set_config(env: &Env, cfg: &AdminConfig) {
    env.storage().instance().set(&DataKey::AdminConfig, cfg);
    env.storage()
        .instance()
        .extend_ttl(CONFIG_TTL_THRESHOLD, CONFIG_TTL_EXTEND);
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
///
/// The configured reward rate scales the incoming `amount` before it is
/// added to the accumulator. Because the rate is applied only to newly
/// notified rewards, a rate change never re-prices rewards that have
/// already been accrued.
fn distribute(env: &Env, token: &Address, amount: i128) {
    let carry_key = DataKey::Undistributed(token.clone());
    let rate = config(env).reward_rate_bps as i128;
    let scaled = amount.checked_mul(rate).unwrap() / MAX_REWARD_RATE_BPS as i128;
    let pending = scaled + get_i128(env, &carry_key);
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
        if config(&env).stakes_blocked {
            panic_with_error!(&env, Error::StakesBlocked);
        }
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

        // The cooldown is snapshotted into the unbond record here, so a
        // later admin cooldown change never affects an in-flight unbond.
        let cooldown = config(&env).cooldown;
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
    ///
    /// While `emergency_exit` is enabled the cooldown check is skipped so
    /// stakers can exit immediately. The pending record is still removed
    /// before the transfer, so a second call cannot double-withdraw, and
    /// no slashing penalty is bypassed (none is applied on this path).
    pub fn withdraw(env: Env, user: Address) -> i128 {
        user.require_auth();
        migrations::require_current(&env);
        let pending_key = DataKey::PendingUnstake(user.clone());
        let pending: PendingUnstake = env
            .storage()
            .persistent()
            .get(&pending_key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NothingToWithdraw));
        if !config(&env).emergency_exit && env.ledger().timestamp() < pending.unlock_at {
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

    // ── Admin controls ──────────────────────────────────────────────────────
    //
    // Every setter is authorized through `common::require_admin_or_multisig`
    // with the dedicated `Auth::StakingAdmin` class and emits a versioned
    // event. Configuration is read through `config()` so the accrual path
    // and the admin path always agree.

    /// Sets the reward rate in basis points of each notified reward that is
    /// distributed to stakers. `0` is allowed (explicitly disables rewards)
    /// and `MAX_REWARD_RATE_BPS` (100%) is the maximum; anything above is
    /// rejected. Applies only to future notifications — already-accrued
    /// rewards are never re-priced.
    pub fn set_reward_rate(env: Env, admin: Address, rate_bps: u32) {
        zenith_common::require_admin_or_multisig(&env, &admin, zenith_common::Auth::StakingAdmin);
        migrations::require_current(&env);
        if rate_bps > MAX_REWARD_RATE_BPS {
            panic_with_error!(&env, Error::InvalidRewardRate);
        }
        let mut cfg = config(&env);
        cfg.reward_rate_bps = rate_bps;
        set_config(&env, &cfg);
        events::reward_rate_set(&env, admin, rate_bps);
    }

    /// Changes the unbonding cooldown applied to *new* unstake requests.
    /// Existing unbond records keep the unlock time snapshotted when they
    /// were created. Bounded by `MAX_COOLDOWN_SECS`.
    pub fn set_cooldown(env: Env, admin: Address, cooldown: u64) {
        zenith_common::require_admin_or_multisig(&env, &admin, zenith_common::Auth::StakingAdmin);
        migrations::require_current(&env);
        if cooldown > MAX_COOLDOWN_SECS {
            panic_with_error!(&env, Error::InvalidCooldown);
        }
        let mut cfg = config(&env);
        cfg.cooldown = cooldown;
        set_config(&env, &cfg);
        events::cooldown_set(&env, admin, cooldown);
    }

    /// Emergency control. `exit = true` lets stakers withdraw pending
    /// unstakes immediately (cooldown bypassed); `block = true` rejects new
    /// stakes. Both flags are independent and can be cleared by passing
    /// `false`. Withdrawals still remove the pending record first, so the
    /// emergency path cannot double-withdraw.
    pub fn set_emergency(env: Env, admin: Address, exit: bool, block: bool) {
        zenith_common::require_admin_or_multisig(&env, &admin, zenith_common::Auth::StakingAdmin);
        migrations::require_current(&env);
        let mut cfg = config(&env);
        cfg.emergency_exit = exit;
        cfg.stakes_blocked = block;
        set_config(&env, &cfg);
        events::emergency_set(&env, admin, exit, block);
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

    pub fn earned(env: Env, user: Address, token: Address) -> i128 {
        let stake = get_i128(&env, &DataKey::Stake(user.clone()));
        let rpt = get_i128(&env, &DataKey::RewardPerToken(token.clone()));
        let paid = get_i128(&env, &DataKey::UserPaid(user.clone(), token.clone()));
        get_i128(&env, &DataKey::Owed(user, token))
            + stake.checked_mul(rpt - paid).unwrap() / ACC_PRECISION
    }

    pub fn staked_of(env: Env, user: Address) -> i128 {
        get_i128(&env, &DataKey::Stake(user))
    }

    pub fn total_staked(env: Env) -> i128 {
        total_staked(&env)
    }

    pub fn pending_unstake(env: Env, user: Address) -> Option<PendingUnstake> {
        env.storage()
            .persistent()
            .get(&DataKey::PendingUnstake(user))
    }

    pub fn reward_tokens(env: Env) -> Vec<Address> {
        reward_tokens(&env)
    }

    pub fn undistributed(env: Env, token: Address) -> i128 {
        get_i128(&env, &DataKey::Undistributed(token))
    }

    pub fn get_splitter(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Splitter).unwrap()
    }

    pub fn get_cooldown(env: Env) -> u64 {
        config(&env).cooldown
    }

    pub fn get_reward_rate(env: Env) -> u32 {
        config(&env).reward_rate_bps
    }

    /// `(emergency_exit, stakes_blocked)`.
    pub fn get_emergency(env: Env) -> (bool, bool) {
        let cfg = config(&env);
        (cfg.emergency_exit, cfg.stakes_blocked)
    }
}
