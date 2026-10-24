use crate::types::Tag;
use soroban_sdk::{Address, Env, Symbol};

pub fn deposited(env: &Env, token: Address, tag: Tag, from: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "deposited"), token, tag), (from, amount));
}

pub fn withdrawn(env: &Env, token: Address, tag: Tag, to: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "withdrawn"), token, tag), (to, amount));
}

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

pub fn swept_untagged(env: &Env, token: Address, to: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "swept_untagged"), token, to), amount);
}

pub fn tag_transferred(env: &Env, token: Address, from_tag: Tag, to_tag: Tag, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "tag_transferred"), token),
        (from_tag, to_tag, amount),
    );
}

pub fn shortfall_detected(env: &Env, token: Address, shortfall: i128) {
    env.events()
        .publish((Symbol::new(env, "shortfall_detected"), token), shortfall);
}

pub fn token_allowed(env: &Env, token: Address, allowed: bool) {
    env.events()
        .publish((Symbol::new(env, "token_allowed"), token), allowed);
}

pub fn integrator_set(env: &Env, integrator: Address, allowed: bool) {
    env.events()
        .publish((Symbol::new(env, "integrator_set"), integrator), allowed);
}

pub fn legacy_migrated(env: &Env, id: u64, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "legacy_migrated"), id), amount);
}
