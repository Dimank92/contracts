use soroban_sdk::{contracttype, Symbol};

/// A registry parameter with its allowed range.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Param {
    /// Current value.
    pub value: i128,
    /// Inclusive lower bound.
    pub min: i128,
    /// Inclusive upper bound.
    pub max: i128,
    /// Ledger timestamp of the last update.
    pub updated_at: u64,
}

/// A queued bounds change for one parameter. Only executable once
/// `eta` has passed — see `BOUNDS_CHANGE_DELAY`.
/// A queued change to a parameter's bounds, executable at `eta`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundsProposal {
    /// Proposed lower bound.
    pub min: i128,
    /// Proposed upper bound.
    pub max: i128,
    /// Earliest execution timestamp.
    pub eta: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Timelock,
    /// Bumped on every value or bounds change, so consumers can cache
    /// values locally and only re-read them when this moves.
    Version,
    Param(Symbol),
    PendingBounds(Symbol),
}
