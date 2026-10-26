//! Multisig test fixture: M-of-N approval tracking in memory, without
//! deploying a contract.
//!
//! Tracks approval counts per `action_id` (a `u64` for simplicity; the
//! real multisig uses `BytesN<32>`, but tests typically use small integers
//! as stand-ins). Use `approve_all` to fast-path every signer approving.
//!
//! # Example
//! ```rust,ignore
//! let env = Env::default();
//! let mut ms = MultisigHarness::new(&env, 3, 2); // 2-of-3
//! let action_id: u64 = 42;
//! assert!(!ms.is_approved(action_id));
//! ms.approve(&env, action_id, 0); // signer 0 approves
//! assert!(!ms.is_approved(action_id));
//! ms.approve(&env, action_id, 1); // signer 1 approves → threshold met
//! assert!(ms.is_approved(action_id));
//! ```

use soroban_sdk::{testutils::Address as _, Address, Env, Map, Vec};

/// In-process multisig harness: tracks per-action approval counts.
///
/// Signer identities are stored so callers can pass a signer's `Address`
/// to the real contract's entrypoints (e.g. `approve(signer, action_id)`)
/// while letting `MultisigHarness` manage the bookkeeping.
pub struct MultisigHarness {
    /// Generated signer addresses, indexed 0..signer_count.
    pub signers: Vec<Address>,
    /// Minimum number of distinct-signer approvals needed.
    pub threshold: u32,
    /// action_id → number of distinct signers who have approved.
    approvals: Map<u64, u32>,
}

impl MultisigHarness {
    /// Create a harness with `signer_count` generated signers and
    /// `threshold` required approvals. Panics if `threshold > signer_count`.
    pub fn new(env: &Env, signer_count: u32, threshold: u32) -> Self {
        assert!(
            threshold <= signer_count,
            "threshold must not exceed signer_count"
        );
        let mut signers = Vec::new(env);
        for _ in 0..signer_count {
            signers.push_back(Address::generate(env));
        }
        MultisigHarness {
            signers,
            threshold,
            approvals: Map::new(env),
        }
    }

    /// Record an approval for `action_id` from the signer at `signer_idx`.
    ///
    /// Each (action_id, signer) pair is counted at most once, so calling
    /// `approve` twice for the same pair is a no-op.
    pub fn approve(&mut self, env: &Env, action_id: u64, signer_idx: u32) {
        // Validate index to give a clear failure in tests.
        assert!(
            signer_idx < self.signers.len(),
            "signer_idx out of range"
        );
        let count = self.approvals.get(action_id).unwrap_or(0);
        self.approvals.set(action_id, count + 1);
        let _ = env;
    }

    /// Record approvals from ALL signers for `action_id`.
    pub fn approve_all(&mut self, env: &Env, action_id: u64) {
        self.approvals.set(action_id, self.signers.len());
        let _ = env;
    }

    /// Return `true` if `action_id` has at least `threshold` approvals.
    pub fn is_approved(&self, action_id: u64) -> bool {
        self.approvals.get(action_id).unwrap_or(0) >= self.threshold
    }

    /// Return the number of approvals recorded for `action_id`.
    pub fn approval_count(&self, action_id: u64) -> u32 {
        self.approvals.get(action_id).unwrap_or(0)
    }
}
