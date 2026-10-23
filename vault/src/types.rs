use soroban_sdk::{contracttype, Address, Symbol};

/// Namespaced escrow tag. `owner` is the integrator whose namespace the
/// tag lives in (e.g. options_market's own address), `kind` separates id
/// spaces inside one integrator (`series` vs `position`), and `id` is the
/// integrator's own identifier. Two integrators — or two kinds inside one
/// integrator — can never collide on the same ledger entry, because the
/// full triple is the storage key.
///
/// Stored as a plain struct rather than a `BytesN<32>` hash: the key is
/// only an address + a short symbol + a u64 larger than a hash, and in
/// exchange events and storage stay self-describing (no preimage registry
/// needed to tell what a hash refers to).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tag {
    pub owner: Address,
    pub kind: Symbol,
    pub id: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    /// The token passed to `initialize`: the default token the `_legacy`
    /// wrappers operate on.
    Token,
    Paused,
    /// Namespace owner that `u64` legacy tags map into — the admin at
    /// initialize time, fixed thereafter so a later transfer_admin can't
    /// orphan legacy balances.
    LegacyOwner,
    /// Token allowlist: only allowed tokens accept new deposits.
    AllowedToken(Address),
    /// Integrator registry: only registered integrators may create tags.
    Integrator(Address),
    /// Per-(token, tag) escrow ledger.
    Escrow(Address, Tag),
    /// Per-token ledger sum.
    TotalEscrowed(Address),
    /// Recorded on a tag's first deposit; gates withdraw/transfer_tag.
    TagOwner(Tag),
    /// Schema v2: every token with a `TotalEscrowed` entry, so migrations
    /// and invariant checks can enumerate the ledger.
    Tokens,
}

/// Storage keys of the pre-multi-token, pre-namespace vault. Encodes
/// identically to the old `DataKey::Escrow(u64)` / `DataKey::TotalEscrowed`
/// so `migrate_legacy` can read (and clear) entries written by it.
#[contracttype]
#[derive(Clone)]
pub enum LegacyKey {
    Escrow(u64),
    TotalEscrowed,
}
