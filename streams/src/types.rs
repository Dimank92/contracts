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
    /// Governance address allowed to cancel any stream.
    Governance,
    StreamCounter,
    Stream(u64),
    /// `(anchor, base)` once a funding operation has rebased the schedule:
    /// `base` was already vested at `anchor`, and the rest streams linearly
    /// from `anchor` to `end`. Absent means `(start, 0)`.
    Checkpoint(u64),
    /// Address the funder allowed to top up / extend this stream.
    Delegate(u64),
}
