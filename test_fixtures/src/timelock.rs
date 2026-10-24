//! Timelock test fixture: an in-process scheduler for unit tests that
//! need timelock semantics without deploying the full timelock contract.
//!
//! Operations are keyed by a monotonically increasing counter and become
//! "ready" once the ledger timestamp reaches `scheduled_at + delay`.
//!
//! # Example
//! ```rust,ignore
//! let env = Env::default();
//! let mut tl = TimelockHarness::new(&env, 3_600); // 1 h min_delay
//! let (op_id, ready_at) = tl.schedule(&env, 3_600);
//! assert!(!tl.is_ready(&env, op_id));
//! env.ledger().with_mut(|l| l.timestamp = ready_at);
//! assert!(tl.is_ready(&env, op_id));
//! tl.cancel(op_id);
//! assert!(!tl.is_ready(&env, op_id));
//! ```

use soroban_sdk::{Env, Map};

/// An in-process timelock harness.
///
/// Operations are stored as `op_id → ready_at_timestamp` in a Soroban
/// `Map`. Cancelling removes the entry; executing is left to the caller.
pub struct TimelockHarness {
    /// The minimum delay enforced by `schedule`.
    pub min_delay: u64,
    /// Pending operations: op_id → ready_at Unix timestamp (seconds).
    operations: Map<u64, u64>,
    /// Next operation id to hand out.
    next_id: u64,
}

impl TimelockHarness {
    /// Create a harness with the given minimum delay (seconds).
    pub fn new(env: &Env, min_delay: u64) -> Self {
        TimelockHarness {
            min_delay,
            operations: Map::new(env),
            next_id: 0,
        }
    }

    /// Schedule an operation with `delay` seconds from now (at least
    /// `min_delay`). Returns `(op_id, ready_at)`.
    pub fn schedule(&mut self, env: &Env, delay: u64) -> (u64, u64) {
        let id = self.next_id;
        self.next_id += 1;
        let effective_delay = delay.max(self.min_delay);
        let ready_at = env.ledger().timestamp() + effective_delay;
        self.operations.set(id, ready_at);
        (id, ready_at)
    }

    /// Return `true` if `op_id` exists and the current ledger timestamp
    /// is at or past its `ready_at`.
    pub fn is_ready(&self, env: &Env, op_id: u64) -> bool {
        self.operations
            .get(op_id)
            .map(|ready_at| env.ledger().timestamp() >= ready_at)
            .unwrap_or(false)
    }

    /// Cancel a pending operation (removes it).
    pub fn cancel(&mut self, op_id: u64) {
        self.operations.remove(op_id);
    }

    /// Return the `ready_at` timestamp for `op_id`, or `None` if it was
    /// never scheduled or has been cancelled.
    pub fn ready_at(&self, op_id: u64) -> Option<u64> {
        self.operations.get(op_id)
    }
}
