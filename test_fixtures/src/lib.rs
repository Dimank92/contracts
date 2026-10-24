#![no_std]

//! Shared test fixtures for the Zenith contracts test suites.
//!
//! This is a plain rlib — no `#[contract]`, no exported spec entries.
//! Import it in your `[dev-dependencies]` with the `testutils` feature:
//!
//! ```toml
//! [dev-dependencies]
//! zenith-test-fixtures = { path = "../test_fixtures", features = ["testutils"] }
//! ```
//!
//! # Quick start
//!
//! ```rust,ignore
//! use zenith_test_fixtures::{multisig::MultisigHarness, oracle::OracleHarness,
//!                             timelock::TimelockHarness, token::TokenFixture};
//!
//! let env = Env::default();
//! env.mock_all_auths();
//!
//! let token = TokenFixture::deploy(&env);
//! let user = Address::generate(&env);
//! token.mint(&env, &user, 1_000_000);
//! assert_eq!(token.balance(&env, &user), 1_000_000);
//!
//! let mut oracle = OracleHarness::new(&env);
//! let feeder = oracle.add_feeder(&env);
//! oracle.report(&env, &feeder, "XLM", 1_200_000);
//! assert_eq!(oracle.get_price(&env, "XLM"), Some(1_200_000));
//! ```

pub mod multisig;
pub mod oracle;
pub mod timelock;
pub mod token;

#[cfg(test)]
mod tests {
    use soroban_sdk::{testutils::Address as _, testutils::Ledger, Env};

    use crate::multisig::MultisigHarness;
    use crate::oracle::OracleHarness;
    use crate::timelock::TimelockHarness;
    use crate::token::TokenFixture;

    #[test]
    fn token_fixture_mint_and_balance() {
        let env = Env::default();
        env.mock_all_auths();
        let token = TokenFixture::deploy(&env);
        let user = soroban_sdk::Address::generate(&env);
        token.mint(&env, &user, 1_000_000);
        assert_eq!(token.balance(&env, &user), 1_000_000);
    }

    #[test]
    fn token_fixture_approve() {
        let env = Env::default();
        env.mock_all_auths();
        let token = TokenFixture::deploy(&env);
        let owner = soroban_sdk::Address::generate(&env);
        let spender = soroban_sdk::Address::generate(&env);
        token.mint(&env, &owner, 500_000);
        token.approve(&env, &owner, &spender, 200_000);
        // Allowance is visible via the standard token client.
        let allowance =
            soroban_sdk::token::Client::new(&env, &token.address).allowance(&owner, &spender);
        assert_eq!(allowance, 200_000);
    }

    #[test]
    fn oracle_harness_single_feeder() {
        let env = Env::default();
        let mut oracle = OracleHarness::new(&env);
        let feeder = oracle.add_feeder(&env);
        oracle.report(&env, &feeder, "XLM", 1_200_000);
        assert_eq!(oracle.get_price(&env, "XLM"), Some(1_200_000));
    }

    #[test]
    fn oracle_harness_quorum_not_met() {
        let env = Env::default();
        let mut oracle = OracleHarness::new(&env);
        oracle.min_reports = 2;
        let feeder = oracle.add_feeder(&env);
        oracle.report(&env, &feeder, "XLM", 1_200_000);
        assert_eq!(oracle.get_price(&env, "XLM"), None);
    }

    #[test]
    fn oracle_harness_stale_report_ignored() {
        let env = Env::default();
        let mut oracle = OracleHarness::new(&env);
        oracle.max_staleness = 60; // 1 minute
        let feeder = oracle.add_feeder(&env);
        oracle.report(&env, &feeder, "XLM", 1_200_000);
        // Advance time past max_staleness.
        env.ledger().with_mut(|l| l.timestamp += 61);
        assert_eq!(oracle.get_price(&env, "XLM"), None);
    }

    #[test]
    fn oracle_harness_median_two_feeders() {
        let env = Env::default();
        let mut oracle = OracleHarness::new(&env);
        oracle.min_reports = 2;
        let f1 = oracle.add_feeder(&env);
        let f2 = oracle.add_feeder(&env);
        oracle.report(&env, &f1, "XLM", 1_000_000);
        oracle.report(&env, &f2, "XLM", 2_000_000);
        // Median of [1_000_000, 2_000_000] = 1_500_000
        assert_eq!(oracle.get_price(&env, "XLM"), Some(1_500_000));
    }

    #[test]
    fn oracle_harness_unauthorized_feeder_ignored() {
        let env = Env::default();
        let mut oracle = OracleHarness::new(&env);
        let _authorized = oracle.add_feeder(&env);
        // Report from a feeder that was never added.
        let stranger = soroban_sdk::Address::generate(&env);
        oracle.report(&env, &stranger, "XLM", 9_999_999);
        assert_eq!(oracle.get_price(&env, "XLM"), None);
    }

    #[test]
    fn multisig_harness_threshold() {
        let env = Env::default();
        let mut ms = MultisigHarness::new(&env, 3, 2);
        let action_id: u64 = 42;
        assert!(!ms.is_approved(action_id));
        ms.approve(&env, action_id, 0);
        assert!(!ms.is_approved(action_id), "one approval below threshold of 2");
        ms.approve(&env, action_id, 1);
        assert!(ms.is_approved(action_id), "two approvals meets threshold of 2");
    }

    #[test]
    fn multisig_harness_approve_all() {
        let env = Env::default();
        let mut ms = MultisigHarness::new(&env, 3, 3);
        let action_id: u64 = 7;
        assert!(!ms.is_approved(action_id));
        ms.approve_all(&env, action_id);
        assert!(ms.is_approved(action_id));
    }

    #[test]
    fn multisig_harness_approval_count() {
        let env = Env::default();
        let mut ms = MultisigHarness::new(&env, 3, 2);
        let action_id: u64 = 1;
        ms.approve(&env, action_id, 0);
        assert_eq!(ms.approval_count(action_id), 1);
        ms.approve(&env, action_id, 1);
        assert_eq!(ms.approval_count(action_id), 2);
    }

    #[test]
    fn timelock_harness_scheduling() {
        let env = Env::default();
        let mut tl = TimelockHarness::new(&env, 3_600);
        let (op_id, ready_at) = tl.schedule(&env, 3_600);
        assert!(!tl.is_ready(&env, op_id), "not ready before delay elapses");
        env.ledger().with_mut(|l| l.timestamp = ready_at);
        assert!(tl.is_ready(&env, op_id), "ready at exact ready_at timestamp");
    }

    #[test]
    fn timelock_harness_min_delay_enforced() {
        let env = Env::default();
        let mut tl = TimelockHarness::new(&env, 3_600);
        // Requesting a delay below min_delay should be clamped up.
        let (_, ready_at) = tl.schedule(&env, 10);
        let now = env.ledger().timestamp();
        assert!(
            ready_at >= now + 3_600,
            "ready_at must be at least min_delay from now"
        );
    }

    #[test]
    fn timelock_harness_cancel() {
        let env = Env::default();
        let mut tl = TimelockHarness::new(&env, 0);
        let (op_id, ready_at) = tl.schedule(&env, 0);
        env.ledger().with_mut(|l| l.timestamp = ready_at);
        assert!(tl.is_ready(&env, op_id));
        tl.cancel(op_id);
        assert!(!tl.is_ready(&env, op_id), "cancelled op is not ready");
    }
}
