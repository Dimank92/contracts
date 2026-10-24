//! Storage TTL policy — see the "Storage TTL policy" table in the repo
//! README. Instance storage is extended at the top of every entrypoint;
//! persistent entries are extended whenever they are read or written
//! through `get_persistent`/`set_persistent`. `extend_ttl` is a no-op
//! (no rent charged) while an entry's remaining TTL is still above the
//! threshold, so the per-call cost only lands once per threshold window.

use soroban_sdk::{Env, IntoVal, TryFromVal, Val};

/// ~5s ledgers.
pub const DAY_IN_LEDGERS: u32 = 17_280;

pub const INSTANCE_BUMP_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
pub const INSTANCE_LIFETIME_THRESHOLD: u32 = INSTANCE_BUMP_AMOUNT - 7 * DAY_IN_LEDGERS;

pub const PERSISTENT_BUMP_AMOUNT: u32 = 90 * DAY_IN_LEDGERS;
pub const PERSISTENT_LIFETIME_THRESHOLD: u32 = PERSISTENT_BUMP_AMOUNT - 30 * DAY_IN_LEDGERS;

pub fn extend_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
}

pub fn extend_persistent<K: IntoVal<Env, Val>>(env: &Env, key: &K) {
    env.storage().persistent().extend_ttl(
        key,
        PERSISTENT_LIFETIME_THRESHOLD,
        PERSISTENT_BUMP_AMOUNT,
    );
}

/// Extends `key` only if it exists — for keeper-driven `bump` calls
/// that may name keys that were never written.
#[allow(dead_code)]
pub fn extend_persistent_if_present<K: IntoVal<Env, Val>>(env: &Env, key: &K) {
    if env.storage().persistent().has(key) {
        extend_persistent(env, key);
    }
}

pub fn get_persistent<K, V>(env: &Env, key: &K) -> Option<V>
where
    K: IntoVal<Env, Val>,
    V: TryFromVal<Env, Val>,
{
    let value = env.storage().persistent().get(key);
    if value.is_some() {
        extend_persistent(env, key);
    }
    value
}

pub fn set_persistent<K, V>(env: &Env, key: &K, value: &V)
where
    K: IntoVal<Env, Val>,
    V: IntoVal<Env, Val>,
{
    env.storage().persistent().set(key, value);
    extend_persistent(env, key);
}
