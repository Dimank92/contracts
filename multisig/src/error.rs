use soroban_sdk::contracterror;

/// Codes are append-only: never renumber or reuse one (docs/errors.md).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    /// The multisig is already initialized.
    /// Cause: calling `initialize` / `initialize_weighted` a second time.
    AlreadyInitialized = 1,
    /// A threshold is invalid for the signer set.
    /// Cause: a threshold of 0 or above the total weight, a rotation threshold not above the threshold (unless unanimous) or above the total weight, or an empty signer set.
    InvalidThreshold = 2,
    /// The address is already a signer.
    /// Cause: initializing with a repeated signer, or a signer change adding an existing signer.
    DuplicateSigner = 3,
    /// The address isn't a signer.
    /// Cause: a signer-only entrypoint called by a non-signer, or a signer change removing a non-signer.
    NotASigner = 4,
    /// This signer already has a fresh approval of this action (or upgrade).
    /// Cause: `approve` / `approve_upgrade` twice by the same signer.
    AlreadyApproved = 5,
    /// The action isn't approved (or this signer has no approval to revoke).
    /// Cause: `execute`, `reset`, `queue_signer_change` or `execute_signer_change` below the required weight, or `revoke` / `revoke_upgrade` without a fresh approval.
    NotYetApproved = 6,
    /// Reserved: the delays are invalid.
    /// Cause: not currently returned.
    InvalidDelays = 7,
    /// Reserved: the custom-account configuration is missing.
    /// Cause: not currently returned.
    AccountNotConfigured = 8,
    /// Reserved: signatures aren't sorted by signer.
    /// Cause: not currently returned.
    UnsortedSignatures = 9,
    /// Reserved: not enough signatures.
    /// Cause: not currently returned.
    InsufficientSignatures = 10,
    /// Reserved: the authorization context isn't allowed.
    /// Cause: not currently returned.
    ContextNotAllowed = 11,
    /// The action is already registered (or queued).
    /// Cause: `register_action` for a pending, executed or vetoed action, or `queue_signer_change` twice.
    ActionAlreadyRegistered = 12,
    /// The action isn't pending.
    /// Cause: approving, executing, vetoing or expiring an action that was executed, reset, expired or vetoed, or never registered.
    ActionNotPending = 13,
    /// The pending-action index is full.
    /// Cause: registering an action when `MAX_PENDING_ACTIONS` are pending, or the proposer already has `MAX_PENDING_PER_SIGNER`.
    TooManyPendingActions = 14,
    /// No proposal has this id.
    /// Cause: `execute` with an id `propose` never returned.
    ProposalNotFound = 15,
    /// The proposal has too many arguments.
    /// Cause: `propose` with more than `MAX_PROPOSAL_ARGS` arguments.
    ProposalTooLarge = 16,
    /// The page or batch size is invalid.
    /// Cause: `get_pending_actions` with a limit of 0 or above `MAX_PAGE_LIMIT`, or `prune` with more than `MAX_PRUNE` ids.
    InvalidPageLimit = 17,
    /// The action can't be expired yet.
    /// Cause: `expire` with no approval TTL configured, before the action is older than it, or while an approval is still fresh.
    NotExpired = 18,
    /// A signer weight is zero.
    /// Cause: initializing or adding a signer with weight 0.
    InvalidWeight = 19,
    /// The total signer weight doesn't fit a `u32`.
    /// Cause: weights summing past `u32::MAX`.
    WeightOverflow = 20,
    /// The signer change is malformed.
    /// Cause: `propose_signer_change` adding or removing more than one signer.
    InvalidSignerChange = 21,
    /// No signer change has this id.
    /// Cause: queueing, vetoing or executing an id `propose_signer_change` never returned.
    SignerChangeNotFound = 22,
    /// The signer change was proposed under an older signer set.
    /// Cause: queueing or executing a change after another change already executed.
    StaleSignerChange = 23,
    /// The rotation delay hasn't elapsed.
    /// Cause: `execute_signer_change` before the queued change's ready time.
    RotationDelayActive = 24,
    /// The signer change isn't queued.
    /// Cause: `execute_signer_change` without a prior `queue_signer_change`.
    SignerChangeNotQueued = 25,
    /// The signer set is too large.
    /// Cause: more than `MAX_SIGNERS` signers.
    TooManySigners = 26,
    /// The caller isn't the vetoer.
    /// Cause: `veto` from an address other than the one `set_vetoer` stored (or before one is set).
    NotVetoer = 27,
    /// The vetoer is already set.
    /// Cause: a second `set_vetoer`.
    VetoerAlreadySet = 28,
    /// The deadline isn't in the future.
    /// Cause: `set_execute_by` with a timestamp at or before now.
    InvalidDeadline = 29,
    /// The caller didn't propose this action.
    /// Cause: `set_execute_by` from someone other than the action's proposer.
    NotProposer = 30,
    /// The action already has a deadline.
    /// Cause: a second `set_execute_by` for the same action.
    DeadlineAlreadySet = 31,
    /// The multisig isn't initialized.
    /// Cause: calling an entrypoint before `initialize`.
    NotInitialized = 32,
    /// Not every signer has approved this wasm.
    /// Cause: `upgrade` when a signer hasn't approved the hash (or a signer was added since unanimity).
    UpgradeNotUnanimous = 33,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    /// `from` isn't the stored schema version.
    /// Cause: `migrate(from, to)` with a stale `from`, including repeating a finished step.
    SchemaVersionMismatch = 100,
    /// The migration target is invalid.
    /// Cause: `migrate` with `to <= from`, or `to` beyond what the running wasm can migrate to.
    InvalidMigrationTarget = 101,
    /// A storage migration is unfinished, so state-changing entrypoints are closed.
    /// Cause: a paginated `migrate` still needs `migrate_batch`, or the wasm was upgraded and `migrate` hasn't run.
    MigrationInProgress = 102,
    /// No paginated migration is in flight.
    /// Cause: `migrate_batch` when nothing is in progress.
    NoMigrationInProgress = 103,
    /// The migration batch is invalid.
    /// Cause: `migrate_batch` with a cursor other than where the last batch stopped, or a limit of 0 or above `MAX_MIGRATION_BATCH`.
    InvalidMigrationBatch = 104,
    // Upgrade safety (zenith_common::upgrade) — same codes in every
    // upgradeable Zenith contract. See docs/upgrades.md.
    /// The wasm hash isn't on the upgrade allowlist.
    /// Cause: `upgrade` / cancel for a hash that was never proposed, or was cancelled or already installed.
    WasmNotProposed = 105,
    /// The allowlisted wasm hash isn't installable yet.
    /// Cause: `upgrade` before the hash's `not_before`.
    UpgradeNotMatured = 106,
    /// The wasm hash is already allowlisted.
    /// Cause: proposing a hash that is already on the allowlist.
    WasmAlreadyProposed = 107,
    /// The post-upgrade self-check hasn't passed, so user-facing entrypoints are closed.
    /// Cause: the wasm was upgraded and `post_upgrade_check` hasn't been called, or it failed (see `upgrade_check_failed`).
    UpgradeCheckPending = 108,
    /// No upgrade is awaiting a self-check.
    /// Cause: `post_upgrade_check` when no upgrade is pending a check.
    NoUpgradeCheckPending = 109,
}
