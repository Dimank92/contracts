#![no_std]

use soroban_sdk::{contracttype, Address, Symbol};

// ─── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    /// The default token set at `initialize` (used by legacy wrappers).
    Token,
    Paused,
    /// Cumulative seconds the vault has spent paused.
    AccruedPauseDuration,
    /// Ledger timestamp at which the current pause began (absent when unpaused).
    PausedAt,
    /// Immutable maximum allowed pause duration in seconds.
    MaxPauseDuration,
    /// Total amount escrowed across all tags for `token`.
    TotalEscrowed(Address),
    /// Per-(token, tag) escrowed balance.
    Escrow(Address, Tag),
    /// Records who owns a tag (the integrator address that opened it).
    TagOwner(Tag),
    /// Who may `emergency_withdraw` the tag once the hatch opens.
    Beneficiary(Tag),
    /// Active-tags index entry — present when a tag has a nonzero balance.
    ActiveTag(u64),
    /// Running counter for the ActiveTag index.
    TagCounter,
    /// Reverse mapping: tag -> its ActiveTag index position.
    TagIndex(Tag),
    // ── allowlists (issue #193) ───────────────────────────────────────────
    /// `true` iff this token address is on the allowlist.
    TokenAllowed(Address),
    /// `true` iff this address is a registered integrator.
    Integrator(Address),
    // ── migration / upgrade ───────────────────────────────────────────────
    SchemaVersion,
    Migration,
    UpgradeMultisig,
}

/// A scoped deposit namespace. `owner` is the integrator contract,
/// `kind` is a short descriptor such as `"series"` or `"position"`,
/// and `id` is the integrator's own row key.
///
/// Two integrators — or two id-spaces inside one integrator — can never
/// collide by construction because the full triple is the storage key.
#[contracttype]
#[derive(Clone, PartialEq, Eq)]
pub struct Tag {
    pub owner: Address,
    pub kind: Symbol,
    pub id: u64,
}
