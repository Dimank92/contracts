use soroban_sdk::{contracttype, Address};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingUnstake {
    pub amount: i128,
    /// Ledger timestamp at which `amount` becomes withdrawable.
    pub unlock_at: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// One-shot initialize flag; see `zenith_common::require_uninitialized`.
    Initialized,
    StakeToken,
    /// The only address allowed to call notify_reward (the fee splitter).
    Splitter,
    Cooldown,
    TotalStaked,
    /// Every reward token ever notified, in first-seen order.
    RewardTokens,
    /// Cumulative reward per staked unit for a token, scaled by
    /// ACC_PRECISION.
    RewardPerToken(Address),
    /// Rewards notified while nothing was staked, carried over to the next
    /// time there is stake to distribute them to.
    Undistributed(Address),
    Stake(Address),
    /// RewardPerToken(token) as of `user`'s last accounting update.
    UserPaid(Address, Address),
    /// Rewards accrued to `user` in `token` but not yet claimed.
    Owed(Address, Address),
    PendingUnstake(Address),
}
