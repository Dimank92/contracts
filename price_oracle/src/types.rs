use soroban_sdk::{contracttype, Address, Bytes, Symbol};

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
    /// Ledgers a slash stays `Disputed` before it can be resolved.
    DisputeWindow,
    SlashDestination,
    /// Number of feeders currently `Disputed`; lets `get_price` skip the
    /// per-feeder bond lookup entirely when it is zero.
    DisputedCount,
    /// Slashed bonds sent to `Burn` / `RewardPool` (accounting totals).
    Burned,
    RewardPool,
    Bond(Address),
    /// Marks `(feeder, report_id)` as already slashed or under dispute.
    SlashedReport(Address, u64),
    /// Latest round id for a symbol.
    LatestRound(Symbol),
    /// Ring-buffer slot `round_id % MAX_ROUNDS`.
    Round(Symbol, u32),
}

/// One retained aggregate round. `cumulative` is the Uniswap-v2-style
/// accumulator `sum(price * elapsed_ledgers)` up to (not including) this
/// round's own interval, in wrapping `u128` arithmetic.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoundData {
    pub round_id: u64,
    pub price: i128,
    pub ledger: u32,
    pub cumulative: u128,
}

/// A TWAP over `window_ledgers`. `gap_ledgers` is the part of the window
/// since the latest accepted round, which was priced at that round's price
/// because no newer round existed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TwapData {
    pub twap: i128,
    pub window_ledgers: u32,
    pub gap_ledgers: u32,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SlashDestination {
    Burn = 0,
    RewardPool = 1,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Resolution {
    Slash = 0,
    Restore = 1,
}

/// An open slash. `amount` is held out of the feeder's active bond.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dispute {
    pub report_id: u64,
    pub amount: i128,
    pub resolve_after: u32,
    pub evidence: Option<Bytes>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BondStatus {
    Active,
    Disputed(Dispute),
    Removed,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bond {
    /// Active stake; excludes any amount held in a dispute.
    pub amount: i128,
    pub status: BondStatus,
}
