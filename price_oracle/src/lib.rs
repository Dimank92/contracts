//! Price Oracle Contract
//!
//! # Storage Key Layout
//!
//! All persistent storage is addressed through the [`DataKey`] enum. Keys are
//! centralized here so that distinct logical data can never share a key and so
//! that the layout stays stable across upgrades.
//!
//! | Variant            | Storage    | Purpose                                              |
//! |--------------------|------------|------------------------------------------------------|
//! | `Admin`            | instance   | Contract admin authorized to configure the oracle.   |
//! | `Base`             | instance   | Base asset the oracle quotes against.                |
//! | `Decimals`         | instance   | Decimal precision used when reporting prices.        |
//! | `Price(Asset)`     | persistent | Latest price for a given asset.                      |
//! | `Timestamp(Asset)` | persistent | Ledger timestamp of the latest price for an asset.   |
//! | `Version`          | instance   | Schema version, reserved for migrations.             |
//!
//! ## Conventions
//! - Every key MUST be a variant of [`DataKey`]; do not construct raw keys inline.
//! - Distinct logical data MUST use distinct variants. Never reuse a variant for
//!   two purposes, even if the payload types happen to match.
//! - Variants carrying an `Asset` are scoped per asset; the asset is part of the
//!   key, so two assets never collide.
//! - `Version` is reserved for schema/migration bookkeeping and MUST NOT be
//!   reused for application data. Future schema keys should be appended as new
//!   variants rather than repurposing existing ones, keeping the layout stable.
//!
//! ## Review checklist for new keys
//! 1. Is the key a new [`DataKey`] variant (not an inline/raw key)?
//! 2. Does it describe exactly one logical datum, distinct from every existing
//!    variant?
//! 3. Is the storage type (instance vs persistent) documented above?
//! 4. Does it avoid the reserved `Version` slot?
//! 5. Is the variant appended (not reordered/renamed) to keep upgrades stable?

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Symbol};

/// Centralized storage keys for the price oracle contract.
///
/// See the module-level documentation for the purpose and storage type of each
/// variant, and for the conventions that prevent key collisions.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Contract admin (instance storage).
    Admin,
    /// Base asset the oracle quotes against (instance storage).
    Base,
    /// Decimal precision for reported prices (instance storage).
    Decimals,
    /// Latest price for an asset (persistent storage).
    Price(Symbol),
    /// Ledger timestamp of the latest price for an asset (persistent storage).
    Timestamp(Symbol),
    /// Schema version, reserved for migrations (instance storage).
    Version,
}

#[contract]
pub struct PriceOracle;

#[contractimpl]
impl PriceOracle {
    /// Initialize the oracle with an admin, base asset, and decimals.
    pub fn initialize(env: Env, admin: Address, base: Symbol, decimals: u32) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Base, &base);
        env.storage().instance().set(&DataKey::Decimals, &decimals);
        env.storage().instance().set(&DataKey::Version, &1u32);
    }

    /// Set the latest price for an asset.
    pub fn set_price(env: Env, asset: Symbol, price: i128) {
        env.storage()
            .persistent()
            .set(&DataKey::Price(asset.clone()), &price);
        env.storage()
            .persistent()
            .set(&DataKey::Timestamp(asset), &env.ledger().timestamp());
    }

    /// Read the latest price for an asset, if any.
    pub fn get_price(env: Env, asset: Symbol) -> Option<i128> {
        env.storage().persistent().get(&DataKey::Price(asset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    #[test]
    fn known_keys_are_distinct() {
        let env = Env::default();
        let a = Symbol::new(&env, "A");
        let b = Symbol::new(&env, "B");

        // Distinct logical data must never share a key.
        assert_ne!(DataKey::Admin, DataKey::Base);
        assert_ne!(DataKey::Base, DataKey::Decimals);
        assert_ne!(DataKey::Decimals, DataKey::Version);
        assert_ne!(DataKey::Price(a.clone()), DataKey::Timestamp(a.clone()));
        assert_ne!(DataKey::Price(a.clone()), DataKey::Price(b.clone()));
        assert_ne!(DataKey::Timestamp(a.clone()), DataKey::Timestamp(b));
    }
}
