use soroban_sdk::{Address, Env, Symbol};

pub fn staked(env: &Env, user: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "staked"), user), amount);
}

pub fn unstake_requested(env: &Env, user: Address, amount: i128, unlock_at: u64) {
    env.events().publish(
        (Symbol::new(env, "unstake_requested"), user),
        (amount, unlock_at),
    );
}

pub fn withdrawn(env: &Env, user: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "withdrawn"), user), amount);
}

pub fn reward_notified(env: &Env, token: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "reward_notified"), token), amount);
}

pub fn reward_claimed(env: &Env, user: Address, token: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "reward_claimed"), user, token), amount);
}

/// Emitted when an admin updates the reward rate.
///
/// Payload: `(old_rate, new_rate)`.
pub fn reward_rate_updated(env: &Env, admin: Address, old_rate: i128, new_rate: i128) {
    env.events().publish(
        (Symbol::new(env, "reward_rate_updated"), admin),
        (old_rate, new_rate),
    );
}

/// Emitted when an admin updates the unbonding cooldown.
///
/// Payload: `(old_cooldown, new_cooldown)` in seconds.
pub fn cooldown_updated(env: &Env, admin: Address, old_cooldown: u64, new_cooldown: u64) {
    env.events().publish(
        (Symbol::new(env, "cooldown_updated"), admin),
        (old_cooldown, new_cooldown),
    );
}

/// Emitted when an admin toggles the emergency mode.
///
/// Payload: `(old_enabled, new_enabled)`.
pub fn emergency_mode_toggled(env: &Env, admin: Address, old_enabled: bool, new_enabled: bool) {
    env.events().publish(
        (Symbol::new(env, "emergency_mode_toggled"), admin),
        (old_enabled, new_enabled),
    );
}