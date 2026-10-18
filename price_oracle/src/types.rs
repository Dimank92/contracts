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
    /// Latest `oracle_adapter::FeederReport` from one feeder for one symbol.
    PriceReport(Symbol, Address),
    /// Unused since the original contract; kept so the variant indices
    /// (and therefore every stored key) stay unchanged.
    AggregatedPrice(Symbol),
    /// The one multisig `*_via_multisig` entrypoints trust, pinned by the
    /// admin with `set_multisig`. Unset means those entrypoints are off.
    TrustedMultisig,
    /// Meaningful decimals (0..=PRICE_DECIMALS) a symbol is quoted with.
    /// A symbol with no entry is unknown and can't be reported.
    SymbolPrecision(Symbol),
}
