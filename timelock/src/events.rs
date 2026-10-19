use soroban_sdk::{Address, BytesN, Env, Symbol};

use crate::types::Role;

/// A batch was scheduled. `ready_at` is the ledger timestamp from which the
/// operation may be executed.
pub fn scheduled(env: &Env, id: BytesN<32>, proposer: Address, ready_at: u64) {
    env.events()
        .publish((Symbol::new(env, "scheduled"), id), (proposer, ready_at));
}

/// A batch executed. Every call in it ran, in order.
pub fn executed(env: &Env, id: BytesN<32>) {
    env.events().publish((Symbol::new(env, "executed"), id), ());
}

/// A pending operation was cancelled and will never run.
pub fn cancelled(env: &Env, id: BytesN<32>, canceller: Address) {
    env.events()
        .publish((Symbol::new(env, "cancelled"), id), canceller);
}

/// The shortest schedulable delay changed.
pub fn min_delay_changed(env: &Env, old: u64, new: u64) {
    env.events()
        .publish((Symbol::new(env, "min_delay_changed"),), (old, new));
}

/// A role was granted or revoked. Both directions are announced: a role
/// quietly disappearing is exactly the thing an operator has to notice.
pub fn role_changed(env: &Env, role: Role, account: Address, granted: bool) {
    env.events().publish(
        (Symbol::new(env, "role_changed"),),
        (role, account, granted),
    );
}
