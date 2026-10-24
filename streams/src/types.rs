use soroban_sdk::{contracttype, Address, Vec};

/// Maximum number of checkpoints allowed in a `Stepped` schedule.
pub const MAX_CHECKPOINTS: u32 = 64;

/// Vesting schedule attached to a stream, selected at creation time.
///
/// All timestamps are ledger timestamps (seconds). The withdrawable amount is
/// a pure function of the current ledger timestamp and the schedule.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Schedule {
    /// Continuous linear vesting from `start` to `end`.
    Linear,
    /// Linear vesting from `start` to `end`, but nothing is withdrawable
    /// before `cliff`. At and after `cliff`, the amount is computed
    /// continuously from `start` (not from `cliff`).
    LinearWithCliff { cliff: u64 },
    /// Discrete milestone releases. `checkpoints` is an ordered list of
    /// `(timestamp, cumulative_amount)` pairs, strictly increasing in both
    /// fields, with the final cumulative amount equal to the stream total.
    Stepped { checkpoints: Vec<(u64, i128)> },
}

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
    ///
    /// Kept in sync with the schedule: for `Linear` this equals `start`,
    /// for `LinearWithCliff` it equals the schedule's cliff, and for
    /// `Stepped` it equals the first checkpoint timestamp.
    pub cliff: u64,
    pub end: u64,
    /// Vesting schedule selected at creation time.
    pub schedule: Schedule,
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
}