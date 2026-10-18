use soroban_sdk::{Address, Env, Symbol};

use crate::types::Tag;

pub fn admin_transferred(env: &Env, old_admin: Address, new_admin: Address) {
    env.events().publish(
        (Symbol::new(env, "admin_transferred"),),
        (old_admin, new_admin),
    );
}

pub fn paused(env: &Env) {
    env.events().publish((Symbol::new(env, "paused"),), ());
}

pub fn unpaused(env: &Env) {
    env.events().publish((Symbol::new(env, "unpaused"),), ());
}

pub fn deposited(env: &Env, token: Address, tag: Tag, from: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "deposited"), token, tag), (from, amount));
}

pub fn withdrawn(env: &Env, token: Address, tag: Tag, to: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "withdrawn"), token, tag), (to, amount));
}

pub fn tag_transferred(
    env: &Env,
    token: Address,
    from_tag: Tag,
    to_tag: Tag,
    amount: i128,
) {
    env.events().publish(
        (Symbol::new(env, "tag_transferred"), token),
        (from_tag, to_tag, amount),
    );
}

pub fn swept_untagged(env: &Env, token: Address, to: Address) {
    env.events()
        .publish((Symbol::new(env, "swept_untagged"), token, to), ());
}

pub fn shortfall_detected(env: &Env, token: Address) {
    env.events()
        .publish((Symbol::new(env, "shortfall_detected"), token), ());
}

pub fn beneficiary_set(env: &Env, tag: Tag, beneficiary: Address) {
    env.events()
        .publish((Symbol::new(env, "beneficiary_set"),), (tag, beneficiary));
}

pub fn emergency_withdrawn(env: &Env, beneficiary: Address, tag: Tag, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "emergency_withdrawn"), beneficiary, tag),
        amount,
    );
}

/// Emitted when a token is added to or removed from the allowlist.
/// `allowed = true` means added, `false` means removed.
pub fn token_allowed(env: &Env, token: Address, allowed: bool) {
    env.events()
        .publish((Symbol::new(env, "token_allowed"),), (token, allowed));
}

/// Emitted when an integrator address is registered or deregistered.
/// `allowed = true` means registered, `false` means deregistered.
pub fn integrator_set(env: &Env, integrator: Address, allowed: bool) {
    env.events()
        .publish((Symbol::new(env, "integrator_set"),), (integrator, allowed));
}
