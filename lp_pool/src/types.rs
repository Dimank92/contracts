use soroban_sdk::{contracttype, Address};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// One-shot initialize flag; see `zenith_common::require_uninitialized`.
    Initialized,
    Admin,
    /// Runs `write_option` and `process_epoch`.
    Keeper,
    /// Collateral/deposit token (USDC).
    Asset,
    /// The options_market this pool writes into.
    Market,
    /// Current epoch number; deposits and withdrawal requests queue into it.
    Epoch,
    /// Pool-owned assets not locked as collateral. Tracked internally —
    /// never read from the token balance — so donations can't move the
    /// share price.
    Idle,
    /// Collateral currently locked in open short positions.
    Locked,
    /// Keeper's mark of what open positions are expected to pay out.
    ExpectedLiabilities,
    TotalShares,
    Shares(Address),
    /// Deposits queued for the current epoch.
    PendingDeposits,
    /// Shares queued for redemption at the end of the current epoch.
    PendingWithdrawShares,
    /// Assets set aside for processed withdrawals not yet claimed.
    Claimable,
    Deposit(Address),
    Withdrawal(Address),
    EpochResult(u32),
    MaxUtilizationBps,
    /// Per-series collateral cap; a series is whitelisted while its cap
    /// is above zero.
    SeriesCap(u64),
    SeriesLocked(u64),
    Positions,
}

/// A user's queued deposit (asset amount) or withdrawal (share amount).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ticket {
    pub epoch: u32,
    pub amount: i128,
}

/// Conversion rates fixed when an epoch is processed; queued tickets
/// from that epoch settle pro rata against them.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpochResult {
    pub deposit_assets: i128,
    pub deposit_shares: i128,
    pub withdraw_shares: i128,
    pub withdraw_assets: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoolPosition {
    pub position_id: u64,
    pub series_id: u64,
    pub collateral: i128,
}
