use soroban_sdk::{contracttype, Address};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stream {
    pub funder: Address,
    pub recipient: Address,
    pub token: Address,
    /// Amount deposited upfront at creation.
    pub total: i128,
    pub start: u64,
    /// Nothing is withdrawable before `cliff`; at `cliff` everything
    /// streamed since `start` unlocks at once.
    pub cliff: u64,
    pub end: u64,
    /// Cumulative amount already paid out to recipients.
    pub withdrawn: i128,
    /// Set once cancelled. The stream is frozen at the amount streamed at
    /// cancellation: `total` is reduced to it and the rest refunded.
    pub canceled: bool,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// One-shot initialize flag; see `zenith_common::require_uninitialized`.
    Initialized,
    /// Governance address allowed to cancel any stream.
    Governance,
    StreamCounter,
    Stream(u64),
}
