#![no_std]

//! Zenith Vault — a per-(token, tag) escrow ledger. See the `vault`
//! reference in the repo README for the trust model.

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env};

mod error;
mod events;
mod ttl;
mod types;

pub mod token_validation;

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_adversaries;

use error::Error;
use token_validation::{TokenError, TokenTransferGuard};
use types::DataKey;
pub use types::Tag;

#[contract]
pub struct Vault;

fn admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}

fn is_registered(env: &Env, integrator: &Address) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::Integrator(integrator.clone()))
}

fn escrow_of(env: &Env, token: &Address, tag: &Tag) -> i128 {
    ttl::get_persistent(env, &DataKey::Escrow(token.clone(), tag.clone())).unwrap_or(0)
}

fn set_escrow(env: &Env, token: &Address, tag: &Tag, amount: i128) {
    ttl::set_persistent(env, &DataKey::Escrow(token.clone(), tag.clone()), &amount);
}

fn total_of(env: &Env, token: &Address) -> i128 {
    ttl::get_persistent(env, &DataKey::TotalEscrowed(token.clone())).unwrap_or(0)
}

fn set_total(env: &Env, token: &Address, amount: i128) {
    ttl::set_persistent(env, &DataKey::TotalEscrowed(token.clone()), &amount);
}

fn tag_owner(env: &Env, tag: &Tag) -> Option<Address> {
    ttl::get_persistent(env, &DataKey::TagOwner(tag.clone()))
}

#[contractimpl]
impl Vault {
    /// One-time setup. `token` becomes the default token and is allowlisted.
    pub fn initialize(env: Env, admin: Address, token: Address, max_pause_duration: u64) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        if max_pause_duration == 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        ttl::extend_instance(&env);
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::MaxPauseDuration, &max_pause_duration);
        ttl::set_persistent(&env, &DataKey::TokenAllowed(token), &true);
    }

    /// Removing a token only blocks new deposits; balances stay movable.
    pub fn set_token_allowed(env: Env, token: Address, allowed: bool) {
        ttl::extend_instance(&env);
        admin(&env).require_auth();
        let key = DataKey::TokenAllowed(token);
        if allowed {
            ttl::set_persistent(&env, &key, &true);
        } else {
            env.storage().persistent().remove(&key);
        }
    }

    /// Registers who may create tags. Deregistering blocks new tags only.
    pub fn set_integrator(env: Env, integrator: Address, allowed: bool) {
        ttl::extend_instance(&env);
        admin(&env).require_auth();
        let key = DataKey::Integrator(integrator);
        if allowed {
            ttl::set_persistent(&env, &key, &true);
        } else {
            env.storage().persistent().remove(&key);
        }
    }

    /// Pulls `amount` of an allowlisted `token` from `from` and credits
    /// `(token, tag)` with the **measured** balance delta, which is
    /// returned. A short delivery (fee-on-transfer, no-op or lying token)
    /// reverts with `TokenDeltaMismatch`. The tag's first deposit also
    /// needs its owner's signature and the owner must be a registered
    /// integrator.
    pub fn deposit(env: Env, from: Address, token: Address, tag: Tag, amount: i128) -> i128 {
        ttl::extend_instance(&env);
        from.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if !env
            .storage()
            .persistent()
            .has(&DataKey::TokenAllowed(token.clone()))
        {
            panic_with_error!(&env, Error::TokenNotAllowed);
        }
        if tag_owner(&env, &tag).is_none() {
            tag.owner.require_auth();
            if !is_registered(&env, &tag.owner) {
                panic_with_error!(&env, Error::UnregisteredIntegrator);
            }
            ttl::set_persistent(&env, &DataKey::TagOwner(tag.clone()), &tag.owner);
        }

        let received = TokenTransferGuard::safe_inbound_transfer(
            &env,
            &token,
            &from,
            &env.current_contract_address(),
            amount,
        )
        .unwrap_or_else(|e| match e {
            TokenError::ZeroTransfer => panic_with_error!(&env, Error::InvalidAmount),
            TokenError::InvalidToken => panic_with_error!(&env, Error::TokenNotAllowed),
            TokenError::DeltaMismatch => panic_with_error!(&env, Error::TokenDeltaMismatch),
        });

        set_escrow(&env, &token, &tag, escrow_of(&env, &token, &tag) + received);
        set_total(&env, &token, total_of(&env, &token) + received);
        events::deposited(&env, token, tag, from, received);
        received
    }

    /// Pays `amount` of `(token, tag)`'s escrow to `to`. Tag owner only.
    pub fn withdraw(env: Env, token: Address, tag: Tag, to: Address, amount: i128) {
        ttl::extend_instance(&env);
        let owner =
            tag_owner(&env, &tag).unwrap_or_else(|| panic_with_error!(&env, Error::UnknownTag));
        owner.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let balance = escrow_of(&env, &token, &tag);
        if balance < amount {
            panic_with_error!(&env, Error::InsufficientEscrowBalance);
        }
        let client = token::Client::new(&env, &token);
        if client.balance(&env.current_contract_address()) < amount {
            panic_with_error!(&env, Error::InsufficientVaultBalance);
        }

        set_escrow(&env, &token, &tag, balance - amount);
        set_total(&env, &token, total_of(&env, &token) - amount);
        client.transfer(&env.current_contract_address(), &to, &amount);
        events::withdrawn(&env, token, tag, to, amount);
    }

    /// Moves `amount` of `token` escrow from `from_tag` to `to_tag` as a
    /// pure ledger move: no token transfer, no cross-contract call, so the
    /// vault's raw balance and `TotalEscrowed` are unchanged and there is
    /// nothing to re-enter. Both tags must belong to the same registered
    /// integrator, who must authorize; a `to_tag` that has never held
    /// escrow is created under that owner. Both tags are validated before
    /// either balance is written.
    pub fn transfer_tag(env: Env, token: Address, from_tag: Tag, to_tag: Tag, amount: i128) {
        ttl::extend_instance(&env);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if from_tag == to_tag {
            panic_with_error!(&env, Error::SameTag);
        }
        if from_tag.owner != to_tag.owner {
            panic_with_error!(&env, Error::CrossOwnerTransfer);
        }
        let owner = tag_owner(&env, &from_tag)
            .unwrap_or_else(|| panic_with_error!(&env, Error::UnknownTag));
        owner.require_auth();
        if !is_registered(&env, &owner) {
            panic_with_error!(&env, Error::UnregisteredIntegrator);
        }
        let from_balance = escrow_of(&env, &token, &from_tag);
        if from_balance < amount {
            panic_with_error!(&env, Error::InsufficientEscrowBalance);
        }
        let to_balance = escrow_of(&env, &token, &to_tag);

        set_escrow(&env, &token, &from_tag, from_balance - amount);
        set_escrow(&env, &token, &to_tag, to_balance + amount);
        if tag_owner(&env, &to_tag).is_none() {
            ttl::set_persistent(&env, &DataKey::TagOwner(to_tag.clone()), &owner);
        }
        events::tag_transferred(&env, token, from_tag, to_tag, amount);
    }

    pub fn balance_of(env: Env, token: Address, tag: Tag) -> i128 {
        ttl::extend_instance(&env);
        escrow_of(&env, &token, &tag)
    }

    pub fn get_total_escrowed(env: Env, token: Address) -> i128 {
        ttl::extend_instance(&env);
        total_of(&env, &token)
    }

    pub fn get_tag_owner(env: Env, tag: Tag) -> Option<Address> {
        ttl::extend_instance(&env);
        tag_owner(&env, &tag)
    }

    pub fn is_token_allowed(env: Env, token: Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::TokenAllowed(token))
    }

    pub fn is_integrator(env: Env, integrator: Address) -> bool {
        is_registered(&env, &integrator)
    }

    pub fn get_admin(env: Env) -> Address {
        admin(&env)
    }

    pub fn get_token(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Token).unwrap()
    }
}
