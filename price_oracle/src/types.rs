use soroban_sdk::{contracttype, Address, Symbol};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Paused,
    /// Freshness window in seconds: a report older than this is ignored
    /// before outlier filtering. Admin-settable, defaults to one hour.
    MaxStaleness,
    /// Minimum number of fresh reports that must survive outlier
    /// filtering before an aggregate is returned or written. Defaults to
    /// 1 at initialize (any single fresh report is enough).
    MinReports,
    /// O(1) membership check — see issue #101. `Feeders` below stays around
    /// only for enumeration (e.g. `get_feeders`), and now lives in
    /// persistent storage instead of instance storage.
    IsFeeder(Address),
    Feeders,
    PriceReport(Symbol, Address),
    /// The latest `PriceRound` written by `aggregate` for a symbol.
    AggregatedPrice(Symbol),
}

/// One successful aggregation, written by `aggregate`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceRound {
    /// 1 for a symbol's first round, then +1 per successful aggregation.
    pub round_id: u64,
    /// Median of the fresh reports that survived outlier filtering.
    pub price: i128,
    pub timestamp: u64,
    /// Fresh reports the price was taken over.
    pub reports_used: u32,
    /// Fresh reports discarded as outliers.
    pub reports_discarded: u32,
}
