use soroban_sdk::{contracttype, Address, Bytes, BytesN, Env, Symbol, Val, Vec};

/// Domain separator mixed into every approval key and signed payload so an
/// approval gathered for one contract instance, class, or network cannot be
/// replayed against the same opaque `action_id` elsewhere.
const APPROVAL_DOMAIN: &[u8] = b"multisig:approval:v1";

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
    /// The `BytesN<32>` is the domain-separated approval key produced by
    /// [`approval_key`], never the raw caller-supplied `action_id`.
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
}

/// Deterministic, domain-separated approval key for `action_id` cast under
/// `class` on this contract instance and network. Callers (and `__check_auth`)
/// must derive their `action_id` and approvals through this single helper so
/// the storage key and the signed payload always agree.
///
/// The key commits to: a fixed domain tag, the contract address, the target
/// class, the network passphrase, and the caller-supplied `action_id`.
/// Approvals are never stored under the raw caller-supplied bytes.
pub fn approval_key(env: &Env, class: &Symbol, action_id: &BytesN<32>) -> BytesN<32> {
    let mut buf = Bytes::new(env);
    buf.extend_from_slice(APPROVAL_DOMAIN);
    use soroban_sdk::xdr::ToXdr;
    buf.append(&env.current_contract_address().to_xdr(env));
    buf.append(&class.to_xdr(env));
    buf.append(&Bytes::from_array(env, &env.ledger().network_id().to_array()));
    buf.append(&Bytes::from_array(env, &action_id.to_array()));
    env.crypto().sha256(&buf).into()
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
    Cancelled = 5,
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

/// The post-upgrade guard (docs/upgrades.md), as `get_upgrade_status`
/// returns it: present from install until `post_upgrade_check` passes.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeStatus {
    pub wasm_hash: BytesN<32>,
    pub installed_at: u64,
    /// The last `post_upgrade_check` failed; the contract stays closed.
    pub failed: bool,
    /// Error code of the failed check (see docs/errors.md), else zero.
    pub reason: u32,
}
