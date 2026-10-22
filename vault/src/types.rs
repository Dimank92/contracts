use soroban_sdk::{contracttype, Address, Symbol};

/// Escrow namespace: the full triple is the storage key, so integrators
/// (and id spaces inside one integrator) can never collide.
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
    /// Default token fixed at `initialize`.
    Token,
    MaxPauseDuration,
    TokenAllowed(Address),
    Integrator(Address),
    /// `(token, tag)` → escrowed balance.
    Escrow(Address, Tag),
    /// token → sum of every tag's balance.
    TotalEscrowed(Address),
    /// Recorded on a tag's first deposit (or first inbound transfer).
    TagOwner(Tag),
}
