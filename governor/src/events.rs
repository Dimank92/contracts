use soroban_sdk::{Address, BytesN, Env, Symbol};

pub fn proposal_created(
    env: &Env,
    id: BytesN<32>,
    proposer: Address,
    snapshot: u32,
    deadline: u32,
) {
    env.events().publish(
        (Symbol::new(env, "proposal_created"), id),
        (proposer, snapshot, deadline),
    );
}

pub fn vote_cast(env: &Env, id: BytesN<32>, voter: Address, support: u32, weight: i128) {
    env.events().publish(
        (Symbol::new(env, "vote_cast"), id, voter),
        (support, weight),
    );
}

pub fn proposal_finalized(env: &Env, id: BytesN<32>, for_votes: i128, against_votes: i128, queued: bool) {
    env.events().publish(
        (Symbol::new(env, "proposal_finalized"), id),
        (for_votes, against_votes, queued),
    );
}

pub fn proposal_queued(env: &Env, id: BytesN<32>, eta: u64) {
    env.events()
        .publish((Symbol::new(env, "proposal_queued"), id), eta);
}

pub fn proposal_executed(env: &Env, id: BytesN<32>, target: Address, eta: u64) {
    env.events().publish(
        (Symbol::new(env, "proposal_executed"), id),
        (target, eta),
    );
}

pub fn proposal_canceled(env: &Env, id: BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "proposal_canceled"), id), ());
}

pub fn param_set(env: &Env, name: &str, value: i128) {
    env.events().publish(
        (Symbol::new(env, "param_set"), Symbol::new(env, name)),
        value,
    );
}