// vault lib.rs — implementation stub.
// The token_validation module is fully implemented (used in tests);
// test modules are declared here.

pub mod token_validation;

#[cfg(test)]
mod test_adversaries;

use soroban_sdk::{contract, contractimpl, contracttype, token, Address, Env, Symbol};

/// Authorization parameter matching every other admin function in the workspace.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Auth {
    Admin,
    Multisig(Address, u32),
}

/// Distinct error variants for the vault contract.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum VaultError {
    TagFrozen = 1,
    TagNotFrozen = 2,
    InsufficientTagBalance = 3,
    Unauthorized = 4,
}

/// Persistent storage keys. The freeze flag is keyed by the same namespaced
/// `(token, tag)` pair used by the escrow ledger so freeze state cannot drift.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    TagBalance(Address, Symbol),
    TagFrozen(Address, Symbol),
}

const DAY_IN_LEDGERS: u32 = 17_280;
const BUMP_AMOUNT: u32 = DAY_IN_LEDGERS * 30;
const BUMP_THRESHOLD: u32 = DAY_IN_LEDGERS * 7;

fn require_admin_or_multisig(env: &Env, auth: &Auth) {
    match auth {
        Auth::Admin => {
            // Admin address is stored under a well-known key by the deployer.
            let admin: Address = env
                .storage()
                .instance()
                .get(&Symbol::new(env, "admin"))
                .expect("admin not set");
            admin.require_auth();
        }
        Auth::Multisig(multisig, _action_id) => {
            multisig.require_auth();
        }
    }
}

fn tag_balance(env: &Env, token: &Address, tag: &Symbol) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::TagBalance(token.clone(), tag.clone()))
        .unwrap_or(0)
}

fn set_tag_balance(env: &Env, token: &Address, tag: &Symbol, amount: i128) {
    let key = DataKey::TagBalance(token.clone(), tag.clone());
    env.storage().persistent().set(&key, &amount);
    env.storage()
        .persistent()
        .extend_ttl(&key, BUMP_THRESHOLD, BUMP_AMOUNT);
}

fn is_frozen(env: &Env, token: &Address, tag: &Symbol) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::TagFrozen(token.clone(), tag.clone()))
        .unwrap_or(false)
}

fn set_frozen(env: &Env, token: &Address, tag: &Symbol, frozen: bool) {
    let key = DataKey::TagFrozen(token.clone(), tag.clone());
    env.storage().persistent().set(&key, &frozen);
    env.storage()
        .persistent()
        .extend_ttl(&key, BUMP_THRESHOLD, BUMP_AMOUNT);
}

#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    /// Freeze a single `(token, tag)` namespace. Deposits and withdrawals for
    /// this tag are rejected; other tags remain fully usable.
    pub fn freeze_tag(env: Env, auth: Auth, token: Address, tag: Symbol) {
        require_admin_or_multisig(&env, &auth);
        set_frozen(&env, &token, &tag, true);
        env.events()
            .publish((Symbol::new(&env, "tag_frozen"),), (token, tag));
    }

    /// Unfreeze a previously frozen `(token, tag)` namespace.
    pub fn unfreeze_tag(env: Env, auth: Auth, token: Address, tag: Symbol) {
        require_admin_or_multisig(&env, &auth);
        set_frozen(&env, &token, &tag, false);
        env.events()
            .publish((Symbol::new(&env, "tag_unfrozen"),), (token, tag));
    }

    /// Deposit into a tag's escrow. Rejected while the tag is frozen.
    pub fn deposit(env: Env, from: Address, token: Address, tag: Symbol, amount: i128) -> Result<(), VaultError> {
        from.require_auth();
        if is_frozen(&env, &token, &tag) {
            return Err(VaultError::TagFrozen);
        }
        token::Client::new(&env, &token).transfer(&from, &env.current_contract_address(), &amount);
        let balance = tag_balance(&env, &token, &tag) + amount;
        set_tag_balance(&env, &token, &tag, balance);
        Ok(())
    }

    /// Withdraw from a tag's escrow. Rejected while the tag is frozen.
    pub fn withdraw(env: Env, to: Address, token: Address, tag: Symbol, amount: i128) -> Result<(), VaultError> {
        to.require_auth();
        if is_frozen(&env, &token, &tag) {
            return Err(VaultError::TagFrozen);
        }
        let balance = tag_balance(&env, &token, &tag);
        if amount > balance {
            return Err(VaultError::InsufficientTagBalance);
        }
        set_tag_balance(&env, &token, &tag, balance - amount);
        token::Client::new(&env, &token).transfer(&env.current_contract_address(), &to, &amount);
        Ok(())
    }

    /// Emergency withdrawal for a frozen tag. Admin/multisig only, only while
    /// frozen, and capped at the tag's accounted balance (never raw balance).
    pub fn emergency_withdraw(
        env: Env,
        auth: Auth,
        token: Address,
        tag: Symbol,
        to: Address,
        amount: i128,
    ) -> Result<(), VaultError> {
        require_admin_or_multisig(&env, &auth);
        if !is_frozen(&env, &token, &tag) {
            return Err(VaultError::TagNotFrozen);
        }
        let balance = tag_balance(&env, &token, &tag);
        if amount > balance {
            return Err(VaultError::InsufficientTagBalance);
        }
        // Decrement accounting so a later unfreeze cannot double-spend.
        set_tag_balance(&env, &token, &tag, balance - amount);
        token::Client::new(&env, &token).transfer(&env.current_contract_address(), &to, &amount);
        env.events().publish(
            (Symbol::new(&env, "emergency_withdraw"),),
            (token, tag, to, amount),
        );
        Ok(())
    }

    /// Query a tag's accounted balance. Remains available while frozen.
    pub fn tag_balance(env: Env, token: Address, tag: Symbol) -> i128 {
        tag_balance(&env, &token, &tag)
    }

    /// Query whether a tag is frozen.
    pub fn is_tag_frozen(env: Env, token: Address, tag: Symbol) -> bool {
        is_frozen(&env, &token, &tag)
    }
}
