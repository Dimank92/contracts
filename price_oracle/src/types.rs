use soroban_sdk::{contracttype, Address, Symbol};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Paused,
    MaxStaleness,
    /// Minimum number of currently-fresh feeder reports get_price requires
    /// before it returns an aggregate at all. Defaults to 1 at initialize
    /// (the original behavior: any single fresh report is enough).
    MinReports,
    /// O(1) membership check — see issue #101. `Feeders` below stays around
    /// only for enumeration (e.g. `get_feeders`), and now lives in
    /// persistent storage instead of instance storage.
    IsFeeder(Address),
    Feeders,
    PriceReport(Symbol, Address),
    AggregatedPrice(Symbol),
    /// Per-symbol metadata: `SymbolMeta { max_staleness_override, decimals }`.
    /// Presence of this key means the symbol is registered. Absence means
    /// it is not supported and `report_price` must reject it.
    SymbolMeta(Symbol),
    /// Number of currently-registered symbols, capped at `MAX_SYMBOLS`.
    SymbolCount,
}

/// Per-symbol metadata registered by the admin.
///
/// - `max_staleness_override`: if `Some`, overrides the contract-wide
///   `max_staleness` for this symbol only (in seconds). `None` means
///   "use the contract-wide setting".
/// - `decimals`: the number of decimal places the price is expressed in
///   (informational, for integrators — e.g. 7 = PRICE_PRECISION).
#[contracttype]
#[derive(Clone)]
pub struct SymbolMeta {
    pub max_staleness_override: Option<u64>,
    pub decimals: u32,
}
