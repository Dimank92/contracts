use soroban_sdk::{Address, Env, Symbol};

pub fn created(env: &Env, stream_id: u64, funder: Address, recipient: Address, total: i128) {
    env.events().publish(
        (Symbol::new(env, "stream_created"), stream_id),
        (funder, recipient, total),
    );
}

pub fn withdrawn(env: &Env, stream_id: u64, to: Address, amount: i128, variant_tag: u32) {
    env.events().publish(
        (Symbol::new(env, "withdrawn"), stream_id),
        (to, amount, variant_tag),
    );
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