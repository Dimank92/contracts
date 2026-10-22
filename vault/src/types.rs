use soroban_sdk::{contracttype, Address};

/// Storage keys for the vault contract.
///
/// # Layout convention
///
/// All vault storage keys MUST be declared in this enum. Do not define
/// ad-hoc keys inline elsewhere in the crate; if a new key is needed, add a
/// variant here so the layout stays centralized and reviewable.
///
/// - `Instance` keys hold contract-wide configuration and are cheap to read
///   on every call. They are rewritten whenever the instance entry is bumped.
/// - `Persistent` keys hold per-user / per-token accounting and must never be
///   reused for a logically distinct datum.
///
/// # Collision prevention
///
/// Distinct logical data must never share a key. Where a single logical
/// record is scoped (e.g. balances are scoped by `(token, tag)`), the scope
/// is encoded in the variant payload rather than by overloading an unrelated
/// variant. Two modules must not pick overlapping variants: the enum is the
/// single source of truth, so adding a variant is the only way to add a key.
///
/// # Reserved ranges
///
/// Variants prefixed with `Version` / `Migration` are reserved for schema
/// versioning and migrations. They must not be repurposed for business data.
/// New business keys are appended after the reserved block so existing
/// discriminants remain stable across upgrades.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Contract admin address. Instance storage.
    Admin,
    /// Underlying token contract address. Instance storage.
    Token,
    /// Share token contract address. Instance storage.
    ShareToken,
    /// Total assets under management. Instance storage.
    TotalAssets,
    /// Total shares outstanding. Instance storage.
    TotalShares,
    /// Per-account share balance, scoped by account. Persistent storage.
    Balance(Address),
    /// Per-account, per-tag asset accounting, scoped by `(token, tag)`.
    /// Persistent storage. The `tag` distinguishes logically distinct
    /// buckets for the same token so they never collide.
    TaggedBalance(Address, u32),
    /// Reserved: current storage schema version. Instance storage.
    /// Do not repurpose for business data.
    Version,
    /// Reserved: migration marker for in-flight upgrades. Instance storage.
    /// Do not repurpose for business data.
    Migration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::Env;

    /// Known keys must be pairwise distinct so no two logical data share a
    /// storage slot. This is a light guard against accidental reuse.
    #[test]
    fn known_keys_are_distinct() {
        let env = Env::default();
        let a = Address::generate(&env);
        let b = Address::generate(&env);

        let keys = [
            DataKey::Admin,
            DataKey::Token,
            DataKey::ShareToken,
            DataKey::TotalAssets,
            DataKey::TotalShares,
            DataKey::Balance(a.clone()),
            DataKey::Balance(b.clone()),
            DataKey::TaggedBalance(a.clone(), 0),
            DataKey::TaggedBalance(a.clone(), 1),
            DataKey::Version,
            DataKey::Migration,
        ];

        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                assert_ne!(keys[i], keys[j], "storage keys must be distinct");
            }
        }
    }
}
