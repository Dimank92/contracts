use soroban_sdk::{Address, BytesN, Env, Symbol};

use crate::types::Role;

/// A batch was scheduled. Topics: `("scheduled", id)`. Data:
/// `(ready_at, predecessor)`.
pub fn scheduled(env: &Env, id: BytesN<32>, ready_at: u64, predecessor: Option<BytesN<32>>) {
    env.events()
        .publish((Symbol::new(env, "scheduled"), id), (ready_at, predecessor));
}

/// A batch ran. Topics: `("executed", id)`. Data: `()`.
pub fn executed(env: &Env, id: BytesN<32>) {
    env.events().publish((Symbol::new(env, "executed"), id), ());
}

/// A batch was cancelled. Topics: `("cancelled", id)`. Data: `()`.
pub fn cancelled(env: &Env, id: BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "cancelled"), id), ());
}

/// The minimum delay changed. Topics: `("min_delay_changed")`. Data:
/// `(old, new)`.
pub fn min_delay_changed(env: &Env, old: u64, new: u64) {
    env.events()
        .publish((Symbol::new(env, "min_delay_changed"),), (old, new));
}

/// A role membership changed. Topics: `("role_changed")`. Data:
/// `(role, account, granted)`.
pub fn role_changed(env: &Env, role: Role, account: Address, granted: bool) {
    env.events().publish(
        (Symbol::new(env, "role_changed"),),
        (role, account, granted),
    );
}

/// Open execution was turned on or off. Topics: `("open_executor_changed")`.
/// Data: `(open)`.
pub fn open_executor_changed(env: &Env, open: bool) {
    env.events()
        .publish((Symbol::new(env, "open_executor_changed"),), open);
}
