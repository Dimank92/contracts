use soroban_sdk::{contracttype, Address, Vec};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingUnstake {
    pub amount: i128,
    /// Ledger timestamp at which `amount` becomes withdrawable.
    pub unlock_at: u64,
}

/// Read-only view of a single staker's position.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    /// Currently staked principal.
    pub staked: i128,
    /// Amount queued for unstake that has not yet been withdrawn.
    pub pending_amount: i128,
    /// Ledger timestamp at which `pending_amount` becomes withdrawable.
    pub pending_unlock_at: u64,
}

/// Read-only view of the contract's current reward configuration.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RewardConfig {
    pub stake_token: Address,
    pub splitter: Address,
    pub cooldown: u64,
    /// Every reward token ever notified, in first-seen order.
    pub reward_tokens: Vec<Address>,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
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
    /// Index of every address that has ever staked, in first-seen order.
    Stakers,
    /// Number of entries in the `Stakers` index.
    StakerCount,
}