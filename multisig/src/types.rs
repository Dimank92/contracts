use soroban_sdk::{contracttype, Address, BytesN, Symbol, Val, Vec};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// O(1) membership check — see issue #101.
    IsSigner(Address),
    /// Map<Address, u32>: every signer and their voting weight.
    Signers,
    /// Approval weight an action needs for `is_approved`.
    Threshold,
    /// How long an approval counts for after it's recorded, in seconds.
    /// Fixed at initialize, immutable — same rationale as Signers and
    /// Threshold. Zero means approvals never expire.
    ApprovalTtl,
    /// Every signer's approval of `action_id` in one entry (bitmap by signer
    /// index in the `Signers` map, plus per-index approval timestamps).
    /// Absent means no approvals. Keyed by the signer-set epoch, so a
    /// rotation invalidates every outstanding vote in O(1). Lives in
    /// temporary storage when `ApprovalTtl > 0`, persistent otherwise.
    Approvals(u32, BytesN<32>),
    /// The one-time vetoer address; immutable once set.
    Vetoer,
    /// action_id -> hard `execute_by` timestamp, independent of the ttl.
    ExecuteBy(BytesN<32>),
    /// action_id -> ActionMeta, the on-chain registry entry.
    Action(BytesN<32>),
    /// Bounded index of every action still awaiting a vote. Entries are
    /// swap-removed on execute, reset, or expiry.
    Pending,
    /// How many entries in `Pending` a given signer registered — bounds
    /// how much of the index any one signer can fill.
    PendingCount(Address),
    /// proposal_id -> Proposal, for actions the multisig executes itself.
    Proposal(BytesN<32>),
    /// Monotonic counter mixed into every proposal so two otherwise
    /// identical calls get distinct ids.
    ProposalNonce,
    /// Approval weight a signer-set change needs. Always above
    /// `Threshold` unless it equals the total weight (unanimity).
    RotationThreshold,
    /// Seconds between a signer change being queued and executable.
    RotationDelay,
    /// Bumped on every executed signer change.
    SignerEpoch,
    /// change_id -> SignerChange.
    SignerChange(BytesN<32>),
    /// change_id -> timestamp at which a queued change becomes executable.
    SignerChangeReadyAt(BytesN<32>),
    /// action class -> seconds of timelock layered on top of `is_approved`
    /// before the action becomes executable. Absent means no timelock.
    ClassTimelock(Symbol),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignerWeight {
    pub address: Address,
    pub weight: u32,
}

/// A proposed signer-set rotation: `add` and `remove` each hold at most
/// one entry. Adding the address being removed changes its weight.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignerChange {
    pub add: Vec<SignerWeight>,
    pub remove: Vec<Address>,
    pub new_threshold: u32,
    pub new_rotation_threshold: u32,
    /// Signer-set epoch the change was proposed in; it can only execute
    /// in that same epoch.
    pub epoch: u32,
    pub nonce: u64,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ActionStatus {
    Pending = 0,
    Executed = 1,
    Reset = 2,
    Expired = 3,
    Vetoed = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionMeta {
    pub proposer: Address,
    pub created_at: u64,
    pub description_hash: BytesN<32>,
    pub status: ActionStatus,
}

/// A call the multisig makes as itself once approved. Its id is the
/// sha256 of its XDR encoding, so the id commits to the exact payload.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub target: Address,
    pub function: Symbol,
    pub args: Vec<Val>,
    pub nonce: u64,
}

/// Per-action approvals in a single ledger entry. `timestamps[i]` is when
/// the signer at index `i` approved; it is only meaningful while bit `i`
/// of `bitmap` is set. Signer indexes are stable within a signer-set epoch.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionApprovals {
    pub bitmap: u32,
    pub timestamps: Vec<u64>,
}

/// Per-class timelock configuration read by `is_executable`. `delay` is in
/// seconds and is measured from the moment the action reached threshold.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassTimelock {
    pub class: Symbol,
    pub delay: u64,
}
