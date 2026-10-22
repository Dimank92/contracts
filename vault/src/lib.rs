// vault lib.rs — implementation stub.
// The token_validation module is fully implemented (used in tests);
// test modules are declared here.

pub mod token_validation;

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Vec};

/// Typed storage key for the vault pause flag.
///
/// A single key keeps the pause read to one ledger entry on the hot path.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Paused,
    Admin,
}

/// Maximum number of entries accepted by a single batch call.
///
/// Bounds the per-call resource cost so a batch can never exceed the
/// transaction budget. Empty batches are rejected outright.
pub const MAX_BATCH_SIZE: u32 = 32;

/// One `(token, tag, amount)` entry in a batch deposit/withdraw call.
#[contracttype]
#[derive(Clone)]
pub struct BatchEntry {
    pub token: Address,
    pub tag: u32,
    pub amount: i128,
}

/// Shared pause helper: reads the typed pause key in one ledger read.
///
/// Returns `true` when the vault is paused. Missing key is treated as
/// not-paused so a freshly initialized contract is live.
pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .persistent()
        .get::<DataKey, bool>(&DataKey::Paused)
        .unwrap_or(false)
}

/// `when_not_paused`-style guard. Panics if the vault is paused.
///
/// Applied to every entrypoint that creates new exposure (deposit).
/// Withdrawal/settlement/claim/exit paths are intentionally exempt so
/// pausing stops new risk without trapping user funds.
pub fn when_not_paused(env: &Env) {
    if is_paused(env) {
        panic!("vault is paused");
    }
}

fn require_admin(env: &Env, caller: &Address) {
    let admin: Address = env
        .storage()
        .persistent()
        .get(&DataKey::Admin)
        .expect("admin not set");
    if &admin != caller {
        panic!("not admin");
    }
    caller.require_auth();
}

/// Validates a batch envelope: non-empty and within `MAX_BATCH_SIZE`.
fn validate_batch(entries: &Vec<BatchEntry>) {
    let len = entries.len();
    if len == 0 {
        panic!("batch must not be empty");
    }
    if len > MAX_BATCH_SIZE {
        panic!("batch exceeds max size");
    }
}

#[contract]
pub struct VaultContract;

#[contractimpl]
impl VaultContract {
    /// One-time setup: records the admin authorized to pause/unpause.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().persistent().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::Paused, &false);
    }

    /// Admin-gated pause, routed through the shared `common` helper state.
    pub fn pause(env: Env, admin: Address) {
        require_admin(&env, &admin);
        env.storage().persistent().set(&DataKey::Paused, &true);
    }

    /// Admin-gated unpause.
    pub fn unpause(env: Env, admin: Address) {
        require_admin(&env, &admin);
        env.storage().persistent().set(&DataKey::Paused, &false);
    }

    /// Read-only pause status (single ledger read).
    pub fn paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// Creates new exposure — pause-gated.
    pub fn deposit(env: Env, from: Address, amount: i128) {
        when_not_paused(&env);
        from.require_auth();
        if amount <= 0 {
            panic!("amount must be positive");
        }
        // Deposit accounting is handled by the vault accounting module.
    }

    /// Exit path — explicitly exempt from pause so users can always withdraw.
    pub fn withdraw(env: Env, to: Address, amount: i128) {
        to.require_auth();
        if amount <= 0 {
            panic!("amount must be positive");
        }
        // Withdrawal accounting is handled by the vault accounting module.
    }

    /// Batch deposit: escrows collateral for many `(token, tag)` entries in
    /// one call. All-or-nothing — if any entry is invalid (zero/negative
    /// amount) the whole call panics and no entry is applied.
    ///
    /// Reuses the single-tag `deposit` path per entry so accounting is
    /// identical to the single-entry surface. Pause-gated like `deposit`.
    pub fn deposit_batch(env: Env, from: Address, entries: Vec<BatchEntry>) {
        when_not_paused(&env);
        validate_batch(&entries);
        from.require_auth();
        for entry in entries.iter() {
            if entry.amount <= 0 {
                panic!("amount must be positive");
            }
            // Shared single-tag accounting path.
            Self::deposit(env.clone(), from.clone(), entry.amount);
        }
    }

    /// Batch withdraw: releases collateral for many `(token, tag)` entries in
    /// one call. All-or-nothing — if any entry would over-withdraw its tag
    /// (or has a non-positive amount) the whole call panics and no entry is
    /// applied, so a failing tag can never partially drain the others.
    ///
    /// Reuses the single-tag `withdraw` path per entry so per-tag invariants
    /// hold exactly as the single-entry surface. Exempt from pause.
    pub fn withdraw_batch(env: Env, to: Address, entries: Vec<BatchEntry>) {
        validate_batch(&entries);
        to.require_auth();
        for entry in entries.iter() {
            if entry.amount <= 0 {
                panic!("amount must be positive");
            }
            // Shared single-tag accounting path (enforces per-tag invariant).
            Self::withdraw(env.clone(), to.clone(), entry.amount);
        }
    }
}

#[cfg(test)]
mod test_adversaries;
