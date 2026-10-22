use soroban_sdk::{contracttype, Address, Symbol, Vec};

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

/// A typed protocol parameter value.
///
/// Stored as a small enum rather than untyped bytes so that reads can
/// fail closed on a type mismatch instead of silently coercing.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParamValue {
    U32(u32),
    I64(i64),
    Bool(bool),
    Address(Address),
}

/// Discriminant tag for a `ParamValue` variant.
///
/// Encoded into the storage key so that two values with the same
/// namespace and key but different types cannot collide.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParamType {
    U32,
    I64,
    Bool,
    Address,
}

impl ParamValue {
    /// Returns the type tag matching this value's variant.
    pub fn type_tag(&self) -> ParamType {
        match self {
            ParamValue::U32(_) => ParamType::U32,
            ParamValue::I64(_) => ParamType::I64,
            ParamValue::Bool(_) => ParamType::Bool,
            ParamValue::Address(_) => ParamType::Address,
        }
    }
}

/// A fully-qualified, namespaced, typed parameter key.
///
/// `namespace` identifies the consuming contract (e.g. `price_oracle`),
/// `key` is the parameter name within that namespace (e.g. `min_reports`),
/// and `type_tag` disambiguates values of different types sharing a name.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParamKey {
    pub namespace: Symbol,
    pub key: Symbol,
    pub type_tag: ParamType,
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
    /// A namespaced, typed parameter value entry.
    Value(ParamKey),
    /// Per-namespace index of registered keys, used for paginated listing.
    NamespaceIndex(Symbol),
    /// Per-namespace count of registered keys.
    NamespaceLen(Symbol),
    /// Per-namespace page of registered keys (paged for bounded storage).
    NamespacePage(Symbol, u32),
}

/// Number of keys stored per namespace index page.
pub const NAMESPACE_PAGE_SIZE: u32 = 32;

/// Helper to build the page index for a given position in a namespace.
pub fn namespace_page_of(position: u32) -> u32 {
    position / NAMESPACE_PAGE_SIZE
}

/// Helper to build the slot within a page for a given position.
pub fn namespace_slot_of(position: u32) -> u32 {
    position % NAMESPACE_PAGE_SIZE
}

/// Convenience alias for a page of keys.
pub type NamespacePage = Vec<ParamKey>;