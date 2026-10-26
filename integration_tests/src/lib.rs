//! Shared fixtures for cross-contract integration tests.
//!
//! Issue #222: this crate is temporarily excluded from the workspace in the
//! root `Cargo.toml` (dated 2024-06-01) because the `options_market`, `vault`,
//! and `timelock` crates do not currently compile. The integration tests that
//! depend on those entrypoints are quarantined with `#[ignore = "..."]`
//! annotations naming the missing entrypoint and the reconstruction issue that
//! will unblock them. Once those crates are reconstructed, remove the workspace
//! exclusion and drop the corresponding `#[ignore]` attributes.
//!
//! Issue #223: the multisig / timelock-tier admin scenario helper lives here so
//! each crate's test is a few lines on top of a shared fixture. The helper is
//! pure ledger bookkeeping (no contract calls) so it compiles independently of
//! the quarantined crates and can be reused once they are reconstructed.

extern crate std;
use std::vec::Vec;
use soroban_sdk::{testutils::Ledger, Address, Env};

/// A single approval record as tracked by the `multisig` crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub signer: Address,
    pub ledger: u32,
}

/// Reusable scenario helper for exercising the `Auth::Multisig` admin path.
///
/// Mirrors the bookkeeping performed by `multisig::approve` / `is_approved` /
/// `is_executable` so tests can assert on threshold, TTL, and class-delay
/// behaviour without reaching into `#[contractimpl]` internals.
pub struct MultisigScenario {
    pub threshold: u32,
    pub approval_ttl: u32,
    pub class_delay: u32,
    pub approvals: Vec<Approval>,
}

impl MultisigScenario {
    pub fn new(threshold: u32, approval_ttl: u32, class_delay: u32) -> Self {
        Self {
            threshold,
            approval_ttl,
            class_delay,
            approvals: Vec::new(),
        }
    }

    /// Record an approval from `signer` at the current ledger sequence.
    pub fn approve(&mut self, env: &Env, signer: Address) {
        self.approvals.push(Approval {
            signer,
            ledger: env.ledger().sequence(),
        });
    }

    /// Number of approvals still within `approval_ttl` at the current ledger.
    ///
    /// An approval is valid while `current - approved_at <= approval_ttl`;
    /// the exact boundary (equal to the TTL) still counts.
    pub fn live_approvals(&self, env: &Env) -> u32 {
        let now = env.ledger().sequence();
        let mut count = 0u32;
        for a in self.approvals.iter() {
            if now.saturating_sub(a.ledger) <= self.approval_ttl {
                count += 1;
            }
        }
        count
    }

    /// `multisig::is_approved` equivalent: live approvals meet the threshold.
    pub fn is_approved(&self, env: &Env) -> bool {
        self.live_approvals(env) >= self.threshold
    }

    /// `multisig::is_executable` equivalent: approved and the class delay has
    /// elapsed since the earliest live approval.
    pub fn is_executable(&self, env: &Env) -> bool {
        if !self.is_approved(env) {
            return false;
        }
        let now = env.ledger().sequence();
        let mut earliest: Option<u32> = None;
        for a in self.approvals.iter() {
            if now.saturating_sub(a.ledger) <= self.approval_ttl {
                earliest = Some(match earliest {
                    Some(e) if e <= a.ledger => e,
                    _ => a.ledger,
                });
            }
        }
        match earliest {
            Some(start) => now.saturating_sub(start) >= self.class_delay,
            None => false,
        }
    }
}

/// Advance the ledger sequence by `delta` (no real sleeps).
pub fn advance_ledger(env: &Env, delta: u32) {
    let next = env.ledger().sequence().saturating_add(delta);
    env.ledger().set_sequence_number(next);
}
