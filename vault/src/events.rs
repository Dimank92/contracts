use soroban_sdk::{Address, Env, Symbol};

pub fn admin_transferred(env: &Env, old: Address, new: Address) {
    env.events()
        .publish((Symbol::new(env, "admin_transferred"),), (old, new));
}

pub fn paused(env: &Env) {
    env.events().publish((Symbol::new(env, "paused"),), ());
}

pub fn unpaused(env: &Env) {
    env.events().publish((Symbol::new(env, "unpaused"),), ());
}

pub fn deposited(env: &Env, token: Address, series_id: u64, from: Address, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "deposited"), token, series_id),
        (from, amount),
    );
}

pub fn withdrawn(env: &Env, token: Address, series_id: u64, to: Address, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "withdrawn"), token, series_id),
        (to, amount),
    );
}
