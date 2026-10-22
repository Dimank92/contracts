use soroban_sdk::{Address, Env, Symbol};

pub fn created(env: &Env, stream_id: u64, funder: Address, recipient: Address, total: i128) {
    env.events().publish(
        (Symbol::new(env, "stream_created"), stream_id),
        (funder, recipient, total),
    );
}

pub fn withdrawn(env: &Env, stream_id: u64, to: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "withdrawn"), stream_id), (to, amount));
}

pub fn canceled(env: &Env, stream_id: u64, to_recipient: i128, to_funder: i128) {
    env.events().publish(
        (Symbol::new(env, "stream_canceled"), stream_id),
        (to_recipient, to_funder),
    );
}

pub fn transferred(env: &Env, stream_id: u64, from: Address, to: Address) {
    env.events().publish(
        (Symbol::new(env, "stream_transferred"), stream_id),
        (from, to),
    );
}

pub fn topped_up(env: &Env, stream_id: u64, from: Address, amount: i128, new_end: u64) {
    env.events().publish(
        (Symbol::new(env, "topped_up"), stream_id),
        (from, amount, new_end),
    );
}

pub fn duration_extended(env: &Env, stream_id: u64, new_end: u64) {
    env.events()
        .publish((Symbol::new(env, "duration_extended"), stream_id), new_end);
}

pub fn rate_changed(env: &Env, stream_id: u64, from: Address, added: i128, new_rate: i128) {
    env.events().publish(
        (Symbol::new(env, "rate_changed"), stream_id),
        (from, added, new_rate),
    );
}
