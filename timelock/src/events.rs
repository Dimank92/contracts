use soroban_sdk::{symbol_short, Address, BytesN, Env};

use crate::types::Role;

pub fn scheduled(env: &Env, id: &BytesN<32>, ready_at: u64, predecessor: &Option<BytesN<32>>) {
    env.events().publish(
        (symbol_short!("scheduled"), id.clone()),
        (ready_at, predecessor.clone()),
    );
}

pub fn executed(env: &Env, id: &BytesN<32>) {
    env.events()
        .publish((symbol_short!("executed"), id.clone()), ());
}

pub fn cancelled(env: &Env, id: &BytesN<32>) {
    env.events()
        .publish((symbol_short!("cancelled"), id.clone()), ());
}

pub fn min_delay_changed(env: &Env, old: u64, new: u64) {
    env.events().publish((symbol_short!("delay"),), (old, new));
}

pub fn role_changed(env: &Env, role: Role, account: &Address, granted: bool) {
    env.events()
        .publish((symbol_short!("role"), role), (account.clone(), granted));
}
