use soroban_sdk::{Env, Symbol};

use crate::types::ParamValue;

pub fn param_defined(env: &Env, key: Symbol, value: i128, min: i128, max: i128) {
    env.events()
        .publish((Symbol::new(env, "param_defined"), key), (value, min, max));
}

pub fn param_updated(env: &Env, key: Symbol, old: i128, new: i128) {
    env.events()
        .publish((Symbol::new(env, "param_updated"), key), (old, new));
}

pub fn bounds_proposed(env: &Env, key: Symbol, min: i128, max: i128, eta: u64) {
    env.events()
        .publish((Symbol::new(env, "bounds_proposed"), key), (min, max, eta));
}

pub fn bounds_updated(env: &Env, key: Symbol, min: i128, max: i128) {
    env.events()
        .publish((Symbol::new(env, "bounds_updated"), key), (min, max));
}

pub fn bounds_cancelled(env: &Env, key: Symbol) {
    env.events()
        .publish((Symbol::new(env, "bounds_cancelled"), key), ());
}

/// Emitted when a namespaced parameter value is set (created or updated).
///
/// Carries the namespace, the key, and the old/new values. When the key was
/// previously unset, `old` is `None`.
pub fn value_set(
    env: &Env,
    namespace: Symbol,
    key: Symbol,
    old: Option<ParamValue>,
    new: ParamValue,
) {
    env.events().publish(
        (Symbol::new(env, "value_set"), namespace, key),
        (old, new),
    );
}

/// Emitted when a namespaced parameter value is removed.
///
/// Carries the namespace, the key, and the value that was removed.
pub fn value_removed(env: &Env, namespace: Symbol, key: Symbol, removed: ParamValue) {
    env.events().publish(
        (Symbol::new(env, "value_removed"), namespace, key),
        removed,
    );
}