use soroban_sdk::{Address, Env, Symbol};

use crate::types::Tag;

pub fn deposited(env: &Env, token: Address, tag: Tag, from: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "deposited"), token, tag), (from, amount));
}

pub fn withdrawn(env: &Env, token: Address, tag: Tag, to: Address, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "withdrawn"), token, tag), (to, amount));
}

pub fn tag_transferred(env: &Env, token: Address, from_tag: Tag, to_tag: Tag, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "tag_transferred"), token),
        (from_tag, to_tag, amount),
    );
}
