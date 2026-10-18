use soroban_sdk::{Address, BytesN, Env, Symbol, Vec};

pub fn scheduled(env: &Env, op_id: BytesN<32>, ready_at: u64, predecessors: Vec<BytesN<32>>) {
    env.events().publish(
        (Symbol::new(env, "scheduled"), op_id),
        (ready_at, predecessors),
    );
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

pub fn canceller_set(env: &Env, canceller: Address, enabled: bool) {
    env.events()
        .publish((Symbol::new(env, "canceller_set"), canceller), enabled);
}

pub fn executor_set(env: &Env, executor: Option<Address>) {
    env.events()
        .publish((Symbol::new(env, "executor_set"),), executor);
}

pub fn min_delay_set(env: &Env, delay: u64) {
    env.events()
        .publish((Symbol::new(env, "min_delay_set"),), delay);
}

pub fn locked_in(env: &Env) {
    env.events().publish((Symbol::new(env, "locked_in"),), ());
}
