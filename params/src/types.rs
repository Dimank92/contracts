use soroban_sdk::{contracttype, Symbol};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Param {
    pub value: i128,
    pub min: i128,
    pub max: i128,
    pub updated_at: u64,
}

/// A queued bounds change for one parameter. Only executable once
/// `eta` has passed — see `BOUNDS_CHANGE_DELAY`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundsProposal {
    pub min: i128,
    pub max: i128,
    pub eta: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// One-shot initialize flag; see `zenith_common::require_uninitialized`.
    Initialized,
    Timelock,
    /// Bumped on every value or bounds change, so consumers can cache
    /// values locally and only re-read them when this moves.
    Version,
    Param(Symbol),
    PendingBounds(Symbol),
}
