use soroban_sdk::{contracttype, Address, Symbol};

// ─── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    /// The settlement oracle: the account allowed to call
    /// `set_settlement_price`, or the price_oracle contract
    /// `set_settlement_price_from_oracle` reads. Re-pointed by `set_oracle`.
    Oracle,
    CollateralToken, // USDC
    FeeRecipient,
    Series(u64),            // series_id -> OptionSeries
    Position(u64),          // position_id -> OptionPosition
    UserPositions(Address), // address -> Vec<u64>
    SeriesCounter,
    PositionCounter,
    UnderlyingPrice(Symbol),
    TotalPremiumsCollected,
    TotalOpenInterest,
    Paused,
    FeeRateBps,
    SeriesCountForUnderlying(Symbol),
    PremiumPool,
    /// Running total of refund-eligible principal still outstanding for
    /// this series: every buy_option/write_option adds its position's own
    /// refund amount (premium net of fee, or full collateral), every
    /// claim_refund/claim_refund_from_vault subtracts what it actually
    /// paid out. Read once by escrow_series_to_vault to size
    /// exactly how much of options_market's own balance to quarantine
    /// into vault for this series — see that function's doc comment.
    SeriesEscrow(u64),
    /// Pinned trusted vault address (`set_vault`). Series created while a
    /// vault is pinned keep their writers' collateral in it.
    Vault,
    /// Records which vault holds escrow for a specific series: series_id ->
    /// Address. Set when the series is created against a pinned vault
    /// (collateral escrowed there from the first write) or, for older
    /// series, by `escrow_series_to_vault` after cancellation.
    SeriesVault(u64),
    /// series_id -> true when the series' writer collateral is escrowed
    /// in `SeriesVault(series_id)` under tag `series_id`, and every payout,
    /// reclaim and collateral refund for it is withdrawn from that tag.
    CollateralInVault(u64),
    /// Optional params registry overriding the fee rate and settlement
    /// window (see storage::sync_params).
    ParamsRegistry,
    /// Registry version the cached fee rate / settlement window came from.
    ParamsVersion,
    SettlementWindow,
}

// ─── Data Types ───────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum OptionType {
    Call, // right to BUY at strike
    Put,  // right to SELL at strike
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum SeriesState {
    Active,    // accepting trades
    Expired,   // past expiry, awaiting settlement
    Settled,   // final settlement price set
    Cancelled, // admin cancelled
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum PositionSide {
    Long,  // bought option (paid premium)
    Short, // wrote option (received premium, locked collateral)
}

/// One option series = one expiry × one strike × one type × one underlying
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct OptionSeries {
    pub series_id: u64,
    pub underlying: Symbol, // "XLM" | "BTC" | "ETH" | "SOL"
    pub option_type: OptionType,
    pub strike_price: i128, // PRICE_PRECISION scale
    pub expiry: u64,        // unix timestamp
    /// Premium per contract (1 contract = 1 unit of underlying, PRICE_PRECISION scale)
    pub premium: i128,
    /// Implied volatility used to price (RATE_PRECISION scale, e.g. 0.45 = 450_000_000)
    pub implied_vol: i128,
    pub open_interest: i128, // total contracts outstanding
    pub state: SeriesState,
    pub settlement_price: Option<i128>,
    pub created_at: u64,
}

/// One user's option position in a series
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct OptionPosition {
    pub position_id: u64,
    pub series_id: u64,
    pub owner: Address,
    pub side: PositionSide,
    pub contracts: i128,         // PRICE_PRECISION scale (1.0 = 10_000_000)
    pub premium_paid: i128,      // total premium paid or received (gross, includes fee for longs)
    pub fee_paid: i128, // protocol fee actually deducted at open time (longs only; 0 for shorts)
    pub collateral_locked: i128, // for writers only
    pub is_exercised: bool,
    pub is_settled: bool,
    pub opened_at: u64,
}
