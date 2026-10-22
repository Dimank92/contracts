//! Storage key layout for the `options_market` contract.
//!
//! All persistent/instance keys used by this crate MUST be declared as
//! variants of [`DataKey`]. Do not construct raw `Symbol`/`Bytes` keys inline
//! anywhere else in the crate — that is exactly how two logically distinct
//! pieces of data end up silently sharing a slot.
//!
//! # Layout
//!
//! | Variant              | Storage    | Purpose                                              |
//! |----------------------|------------|------------------------------------------------------|
//! | `Admin`              | instance   | Contract admin address.                              |
//! | `Config`             | instance   | Global market configuration (fees, caps, etc.).      |
//! | `SeriesCount`        | instance   | Monotonic counter used to allocate series ids.       |
//! | `Series(u64)`        | persistent | Per-series metadata, scoped by series id.            |
//! | `SeriesState(u64)`   | persistent | Per-series mutable state (open/expired/settled).     |
//! | `Position(u64, u32)` | persistent | Per-series position for a given holder index.        |
//! | `Version`            | instance   | Schema version, used by the migration scheme.        |
//!
//! # Conventions
//!
//! * **One logical datum, one variant.** Never reuse a variant for a second
//!   meaning; add a new variant instead. Series-scoped data always carries the
//!   `series_id` as its first field so distinct series can never collide.
//! * **Instance vs persistent is part of the contract.** Moving a key between
//!   the two storage types is a breaking schema change and requires a
//!   migration (see `docs/migrations.md`).
//! * **Reserved range.** Variants prefixed `Reserved` are reserved for future
//!   schema keys and must not be used until a migration claims them. The
//!   `Version` key is reserved for the migration scheme and must never be
//!   repurposed for application data.
//! * **Stability across upgrades.** Existing variants keep their names and
//!   field order forever; new data is added as new variants only.
//!
//! # Review checklist for new keys
//!
//! 1. Is the datum already covered by an existing variant? Reuse it.
//! 2. Does the variant name describe exactly one logical datum?
//! 3. Is series-scoped data keyed by `series_id` first?
//! 4. Is the storage type (instance vs persistent) justified and documented?
//! 5. Does adding it require a `Version` bump / migration?
//! 6. Is there a distinctness assertion covering the new variant?

use soroban_sdk::{contracttype, Address};

/// Canonical storage key for the `options_market` contract.
///
/// See the module-level docs for the purpose and storage type of each variant.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Contract admin address. Instance storage.
    Admin,
    /// Global market configuration. Instance storage.
    Config,
    /// Monotonic series-id allocator. Instance storage.
    SeriesCount,
    /// Per-series metadata, scoped by series id. Persistent storage.
    Series(u64),
    /// Per-series mutable state, scoped by series id. Persistent storage.
    SeriesState(u64),
    /// Per-series position for a holder index. Persistent storage.
    Position(u64, u32),
    /// Schema version for the migration scheme. Instance storage. Reserved.
    Version,
    /// Reserved for future schema keys. Do not use without a migration.
    Reserved(u32),
}

/// Convenience alias used by call sites that previously referenced the
/// contract's storage key type by name.
pub type StorageKey = DataKey;

/// Marker kept for call sites that need the admin address type alongside keys.
pub type AdminAddress = Address;

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    /// Known keys must be pairwise distinct so no two logical data slots can
    /// silently collide.
    #[test]
    fn known_keys_are_distinct() {
        let env = Env::default();
        let keys = [
            DataKey::Admin,
            DataKey::Config,
            DataKey::SeriesCount,
            DataKey::Series(0),
            DataKey::SeriesState(0),
            DataKey::Position(0, 0),
            DataKey::Version,
            DataKey::Reserved(0),
        ];

        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                assert_ne!(
                    keys[i], keys[j],
                    "storage keys at index {i} and {j} collide"
                );
            }
        }

        // Series-scoped keys must not collide across series ids.
        assert_ne!(DataKey::Series(0), DataKey::Series(1));
        assert_ne!(DataKey::SeriesState(0), DataKey::SeriesState(1));
        assert_ne!(DataKey::Position(0, 0), DataKey::Position(0, 1));

        // Touch the env so the test harness is exercised consistently.
        let _ = env.ledger().sequence();
    }
}
