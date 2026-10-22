use soroban_sdk::{Address, Env, Symbol};

pub fn proposed(env: &Env, id: u64, proposer: Address, execute_after: u64) {
    env.events()
        .publish((Symbol::new(env, "proposed"), id, proposer), execute_after);
}

pub fn executed(env: &Env, id: u64) {
    env.events().publish((Symbol::new(env, "executed"), id), ());
}

pub fn cancelled(env: &Env, id: u64) {
    env.events()
        .publish((Symbol::new(env, "cancelled"), id), ());
}
