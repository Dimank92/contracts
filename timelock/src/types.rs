use soroban_sdk::{contracttype, Address, BytesN, Symbol, Val, Vec};

/// What a participant in the timelock is allowed to do.
///
/// Three roles, and no admin: the role sets are the whole configuration, and
/// the timelock's own settings change only through a delayed operation.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Role {
    /// May schedule operations.
    Proposer = 0,
    /// May execute a ready operation.
    Executor = 1,
    /// May cancel a pending operation.
    Canceller = 2,
}

/// A scheduled operation, stored under its derived id.
///
/// The three parallel vectors describe the batch: `targets[i]` is the contract
/// call `i` goes to, `fns[i]` the function it calls, and `args[i]` the
/// arguments it is called with. They are always the same length.
#[contracttype]
#[derive(Clone)]
pub struct Operation {
    pub targets: Vec<Address>,
    pub fns: Vec<Symbol>,
    pub args: Vec<Vec<Val>>,
    /// The operation that must have executed before this one may execute,
    /// or the zero id when there is none.
    ///
    /// `Option<BytesN<32>>` cannot be a `#[contracttype]` field — the SDK's
    /// XDR conversion has no `From<Option<..>>` for it — so the stored form is
    /// the id itself, with the zero id standing for "no predecessor".
    pub predecessor: BytesN<32>,
    /// The nonce that makes two otherwise identical batches distinct.
    pub salt: BytesN<32>,
    /// Ledger timestamp from which the operation may be executed.
    pub ready_at: u64,
    /// Set once the operation has executed; the record is kept so a caller
    /// can still tell "done" from "never existed".
    pub done: bool,
}

/// The preimage an operation id is derived from.
///
/// The id commits to exactly what will run: the calls, the predecessor they
/// wait on, and the salt that makes two otherwise identical batches distinct.
/// The parts of an operation that change over its life — when it became
/// ready, whether it has run — are deliberately not part of it, so the id can
/// be computed before the operation is scheduled.
#[contracttype]
#[derive(Clone)]
pub struct OperationSpec {
    pub targets: Vec<Address>,
    pub fns: Vec<Symbol>,
    pub args: Vec<Vec<Val>>,
    /// The predecessor they wait on, or the zero id when there is none.
    pub predecessor: BytesN<32>,
    pub salt: BytesN<32>,
}

/// Storage keys.
///
/// The role sets are one key per (role, address) pair, so a membership check
/// is a single storage read and a revocation is a single removal.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Set once `initialize` has run. The timelock has no admin afterwards.
    Initialized,
    /// The shortest delay any operation may be scheduled with, in seconds.
    MinDelay,
    /// How long a ready operation stays executable, in seconds.
    GracePeriod,
    /// True when anyone may execute a ready operation.
    OpenExecutor,
    Proposer(Address),
    Executor(Address),
    Canceller(Address),
    /// A scheduled operation, keyed by its derived id.
    Operation(BytesN<32>),
}
