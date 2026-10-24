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

/// Terminal/lifecycle status reported by query entrypoints.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamStatus {
    /// Before `start` (or before `cliff`): nothing has vested yet.
    Pending,
    /// Between `start`/`cliff` and `end`: actively vesting.
    Streaming,
    /// Past `end`: fully vested.
    Completed,
    /// Cancelled by governance; frozen at the amount streamed at cancellation.
    Cancelled,
}

/// Full read-only view of a stream returned by `get_stream`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamRecord {
    pub sender: Address,
    pub recipient: Address,
    pub token: Address,
    pub total: i128,
    pub start: u64,
    pub cliff: u64,
    pub duration: u64,
    pub withdrawn: i128,
    pub status: StreamStatus,
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
    /// Per-sender enumeration index: (sender, stream_id) -> ()
    SenderStream(Address, u64),
    /// Per-recipient enumeration index: (recipient, stream_id) -> ()
    RecipientStream(Address, u64),
}
