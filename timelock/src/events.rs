use soroban_sdk::{Address, BytesN, Env, Symbol};

pub fn scheduled(env: &Env, op_id: BytesN<32>, ready_at: u64) {
    env.events()
        .publish((Symbol::new(env, "scheduled"), op_id), ready_at);
}

pub fn executed(env: &Env, op_id: BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "executed"), op_id), ());
}

pub fn canceled(env: &Env, op_id: BytesN<32>, by: Address) {
    env.events()
        .publish((Symbol::new(env, "canceled"), op_id), by);
}

pub fn proposer_set(env: &Env, old: Address, new: Address) {
    env.events()
        .publish((Symbol::new(env, "proposer_set"),), (old, new));
}

pub fn locked_in(env: &Env) {
    env.events().publish((Symbol::new(env, "locked_in"),), ());
}

pub fn executor_set(env: &Env, executor: Option<Address>) {
    env.events()
        .publish((Symbol::new(env, "executor_set"),), executor);
}
