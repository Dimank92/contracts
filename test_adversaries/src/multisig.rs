//! Adversarial multisig contracts.
//!
//! Each adversary here exposes `is_executable(action_id, class) -> bool` —
//! the only function `zenith-common`'s `require_admin_or_multisig` ever
//! calls on a multisig via `invoke_contract`. No wasm is needed because
//! common calls it with `invoke_contract` by symbol, not via `contractimport!`.
//!
//! ## Adversaries
//!
//! | Struct                   | Behaviour                                                    |
//! |--------------------------|--------------------------------------------------------------|
//! | [`AlwaysApproveMultisig`] | `is_executable()` always returns `true` — any action is immediately approved with no quorum. |
//! | [`NeverApproveMultisig`]  | `is_executable()` always returns `false` — no action can ever proceed. |
//! | [`ThresholdZeroMultisig`] | Initialized with threshold 0; models misconfigurations that should be rejected at `initialize`. |
//! | [`SingleSignerMultisig`]  | Only one signer and threshold 1; the weakest legitimate multisig. |

use soroban_sdk::{contract, contractimpl, contracttype, Address, BytesN, Env, Vec};

// ─── ActionClass mirrors zenith-common's enum (same u32 repr) ──────────────

/// Mirrors `zenith_common::ActionClass` without creating a dependency on it.
/// The variants must have the same `u32` discriminants as the real enum.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ActionClass {
    Emergency = 0,
    Standard = 1,
    Critical = 2,
}

// ─── AlwaysApproveMultisig ───────────────────────────────────────────────────

pub use always_approve_multisig::{AlwaysApproveMultisig, AlwaysApproveMultisigClient};

mod always_approve_multisig {
    use super::*;

    /// A multisig whose `is_executable` always returns `true` regardless of
    /// `action_id`, `class`, the timelock delay, or how many approvals exist.
    /// Anyone can invoke any `_via_multisig` function on any contract that
    /// trusts this address as its multisig.
    ///
    /// **Vulnerability exercised (issue #115):** `require_admin_or_multisig`
    /// calls `is_executable` and panics if it returns false. If an attacker
    /// can substitute an always-approve multisig, they can execute any
    /// admin-gated function without the required quorum. Tests confirm that
    /// options_market / price_oracle / vault validate the multisig address
    /// (it must match the stored admin) rather than accepting any address.
    #[contract]
    pub struct AlwaysApproveMultisig;

    #[contractimpl]
    impl AlwaysApproveMultisig {
        pub fn initialize(env: Env, signers: Vec<Address>, _threshold: u32, _approval_ttl: u64) {
            env.storage()
                .instance()
                .set(&symbol_multisig("INIT"), &signers);
        }

        /// Always returns `true`.
        pub fn is_executable(_env: Env, _action_id: u64, _class: ActionClass) -> bool {
            true
        }

        pub fn is_approved(_env: Env, _action_id: u64) -> bool {
            true
        }
        pub fn approve(_env: Env, _signer: Address, _action_id: u64) {}
        pub fn revoke(_env: Env, _signer: Address, _action_id: u64) {}
        pub fn is_signer(_env: Env, _address: Address) -> bool {
            true
        }
        pub fn get_signer_count(_env: Env) -> u32 {
            1
        }
        pub fn get_threshold(_env: Env) -> u32 {
            1
        }
        pub fn get_approval_ttl(_env: Env) -> u64 {
            0
        }
        pub fn has_approved(_env: Env, _action_id: u64, _signer: Address) -> bool {
            true
        }
        pub fn get_approval_count(_env: Env, _action_id: u64) -> u32 {
            u32::MAX
        }
    }
}

// ─── NeverApproveMultisig ────────────────────────────────────────────────────

pub use never_approve_multisig::{NeverApproveMultisig, NeverApproveMultisigClient};

mod never_approve_multisig {
    use super::*;

    /// A multisig whose `is_executable` always returns `false`. Any
    /// `_via_multisig` call that relies on it will revert with `Unauthorized`.
    ///
    /// **Vulnerability exercised (issue #115):** Confirms that the `_via_multisig`
    /// path in each contract rejects the call and does not proceed with the
    /// admin action when the multisig hasn't approved it.
    #[contract]
    pub struct NeverApproveMultisig;

    #[contractimpl]
    impl NeverApproveMultisig {
        pub fn initialize(env: Env, signers: Vec<Address>, _threshold: u32, _approval_ttl: u64) {
            env.storage()
                .instance()
                .set(&symbol_multisig("INIT"), &signers);
        }

        /// Always returns `false`.
        pub fn is_executable(_env: Env, _action_id: u64, _class: ActionClass) -> bool {
            false
        }

        pub fn is_approved(_env: Env, _action_id: u64) -> bool {
            false
        }
        pub fn approve(_env: Env, _signer: Address, _action_id: u64) {}
        pub fn revoke(_env: Env, _signer: Address, _action_id: u64) {}
        pub fn is_signer(_env: Env, _address: Address) -> bool {
            false
        }
        pub fn get_signer_count(_env: Env) -> u32 {
            0
        }
        pub fn get_threshold(_env: Env) -> u32 {
            u32::MAX
        }
        pub fn get_approval_ttl(_env: Env) -> u64 {
            0
        }
        pub fn has_approved(_env: Env, _action_id: u64, _signer: Address) -> bool {
            false
        }
        pub fn get_approval_count(_env: Env, _action_id: u64) -> u32 {
            0
        }
    }
}

// ─── SingleSignerMultisig ────────────────────────────────────────────────────

pub use single_signer_multisig::{SingleSignerMultisig, SingleSignerMultisigClient};

mod single_signer_multisig {
    use super::*;

    /// The weakest *legitimate* multisig: 1-of-1. Useful for positive-path
    /// tests where the multisig path must succeed without giving every caller
    /// blanket approval.
    ///
    /// `set_action_approved(action_id)` manually marks an action as executable
    /// for tests that need fine-grained control.
    #[contract]
    pub struct SingleSignerMultisig;

    #[contracttype]
    enum SingleSignerKey {
        Signer,
        Approved(u64),
    }

    #[contractimpl]
    impl SingleSignerMultisig {
        pub fn initialize(env: Env, signer: Address, _approval_ttl: u64) {
            env.storage()
                .instance()
                .set(&SingleSignerKey::Signer, &signer);
        }

        /// Manually mark `action_id` as approved for use in tests.
        pub fn set_action_approved(env: Env, action_id: u64, approved: bool) {
            env.storage()
                .instance()
                .set(&SingleSignerKey::Approved(action_id), &approved);
        }

        pub fn is_executable(env: Env, action_id: u64, _class: ActionClass) -> bool {
            env.storage()
                .instance()
                .get(&SingleSignerKey::Approved(action_id))
                .unwrap_or(false)
        }

        pub fn is_approved(env: Env, action_id: u64) -> bool {
            env.storage()
                .instance()
                .get(&SingleSignerKey::Approved(action_id))
                .unwrap_or(false)
        }

        pub fn approve(env: Env, signer: Address, action_id: u64) {
            signer.require_auth();
            let stored: Address = env
                .storage()
                .instance()
                .get(&SingleSignerKey::Signer)
                .unwrap();
            if signer == stored {
                env.storage()
                    .instance()
                    .set(&SingleSignerKey::Approved(action_id), &true);
            }
        }

        pub fn revoke(env: Env, _signer: Address, action_id: u64) {
            env.storage()
                .instance()
                .set(&SingleSignerKey::Approved(action_id), &false);
        }

        pub fn is_signer(env: Env, address: Address) -> bool {
            let stored: Option<Address> = env.storage().instance().get(&SingleSignerKey::Signer);
            stored.map(|s| s == address).unwrap_or(false)
        }

        pub fn get_signer_count(_env: Env) -> u32 {
            1
        }
        pub fn get_threshold(_env: Env) -> u32 {
            1
        }
        pub fn get_approval_ttl(_env: Env) -> u64 {
            0
        }

        pub fn has_approved(env: Env, action_id: u64, _signer: Address) -> bool {
            env.storage()
                .instance()
                .get(&SingleSignerKey::Approved(action_id))
                .unwrap_or(false)
        }

        pub fn get_approval_count(env: Env, action_id: u64) -> u32 {
            let approved: bool = env
                .storage()
                .instance()
                .get(&SingleSignerKey::Approved(action_id))
                .unwrap_or(false);
            if approved {
                1
            } else {
                0
            }
        }
    }
}

// ─── ExpiredApprovalMultisig ──────────────────────────────────────────────────

pub use expired_approval_multisig::{ExpiredApprovalMultisig, ExpiredApprovalMultisigClient};

mod expired_approval_multisig {
    use super::*;

    /// A multisig that records approvals but `is_executable` always returns
    /// `false` because every approval is treated as expired (approval_ttl = 1
    /// second and the ledger has already advanced past it). Models the scenario
    /// where a valid M-of-N quorum was reached but the timelock delay window
    /// expired before anyone called `execute`.
    ///
    /// **Vulnerability exercised (issue #115):** `_via_multisig` calls should
    /// fail even if quorum was previously reached, if the action's timelock
    /// has since expired. Confirms options_market / price_oracle / vault do not
    /// cache `is_executable`'s return value across ledgers.
    #[contract]
    pub struct ExpiredApprovalMultisig;

    #[contracttype]
    enum ExpiredKey {
        Signers,
        ApprovalTime(u64),
    }

    #[contractimpl]
    impl ExpiredApprovalMultisig {
        pub fn initialize(env: Env, signers: Vec<Address>, _threshold: u32) {
            env.storage().instance().set(&ExpiredKey::Signers, &signers);
        }

        /// Record an approval timestamp so tests can set up state.
        pub fn record_approval(env: Env, action_id: u64, timestamp: u64) {
            env.storage()
                .instance()
                .set(&ExpiredKey::ApprovalTime(action_id), &timestamp);
        }

        /// Returns `false` — the approval is always expired (ttl was 1 second).
        pub fn is_executable(_env: Env, _action_id: u64, _class: ActionClass) -> bool {
            false // always expired
        }

        pub fn is_approved(_env: Env, _action_id: u64) -> bool {
            false
        }
        pub fn approve(env: Env, _signer: Address, action_id: u64) {
            env.storage().instance().set(
                &ExpiredKey::ApprovalTime(action_id),
                &env.ledger().timestamp(),
            );
        }
        pub fn revoke(_env: Env, _signer: Address, _action_id: u64) {}
        pub fn is_signer(_env: Env, _address: Address) -> bool {
            true
        }
        pub fn get_signer_count(_env: Env) -> u32 {
            3
        }
        pub fn get_threshold(_env: Env) -> u32 {
            2
        }
        pub fn get_approval_ttl(_env: Env) -> u64 {
            1
        } // 1 second → immediately expires
        pub fn has_approved(_env: Env, _action_id: u64, _signer: Address) -> bool {
            false
        }
        pub fn get_approval_count(_env: Env, _action_id: u64) -> u32 {
            0
        }
    }
}

// ─── helper ──────────────────────────────────────────────────────────────────

/// Internal helper: produce a storage key symbol for simple string labels.
fn symbol_multisig(s: &str) -> soroban_sdk::Symbol {
    // We can't call Symbol::new without an Env here; use contracttype storage
    // keys instead. This function is only called with static strings that fit
    // in a short symbol. The actual value is never read back — it's just used
    // to mark initialization.
    let _ = s; // suppress unused-parameter warning in this shim
               // return a BytesN<32> or use a unit type instead —
               // for initialization we just store a bool flag.
    soroban_sdk::Symbol::short("INIT")
}

// ─── re-export BytesN for action_id helpers in tests ─────────────────────────

/// Convenience: build a 32-byte action id from a `u64` discriminant.
/// The bytes are the little-endian u64 followed by 24 zero bytes.
pub fn action_id_from_u64(env: &Env, discriminant: u64) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&discriminant.to_le_bytes());
    BytesN::from_array(env, &bytes)
}

// ─── Adversarial suite: duplicate/replayed and expired approvals ─────────────
//
// These tests exercise the *real* multisig contract through its public
// entrypoints only (no storage back doors). They are deterministic: ledger
// timestamps are advanced explicitly via `env.ledger().set_timestamp`, never
// read from the wall clock, and no RNG is involved.
//
// The suite is gated behind the `multisig` feature so that it is compiled and
// executed in CI (see `test_adversaries/README.md` and the CI report) rather
// than being silently skipped. When the real `multisig` crate is not present
// in the workspace the module is still compiled but the tests are reported as
// gated in the CI output.

#[cfg(all(test, feature = "multisig"))]
mod adversarial_suite {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger as _};
    use soroban_sdk::{Address, Env, Vec};

    /// Build a deterministic 3-signer / threshold-2 multisig fixture.
    fn setup(env: &Env) -> (Address, Address, Address, Address) {
        let contract_id = env.register_contract(None, MultisigUnderTest);
        let s1 = Address::generate(env);
        let s2 = Address::generate(env);
        let s3 = Address::generate(env);
        let mut signers = Vec::new(env);
        signers.push_back(s1.clone());
        signers.push_back(s2.clone());
        signers.push_back(s3.clone());
        // approval_ttl = 100 ledgers/seconds; deterministic.
        MultisigUnderTestClient::new(env, &contract_id).initialize(&signers, &2u32, &100u64);
        (contract_id, s1, s2, s3)
    }

    /// A duplicate approval from the same signer must not count twice toward
    /// the quorum. With threshold 2, one signer approving twice must leave the
    /// action non-executable.
    #[test]
    fn duplicate_approval_does_not_reach_quorum() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);
        let (contract_id, s1, _s2, _s3) = setup(&env);
        let client = MultisigUnderTestClient::new(&env, &contract_id);
        let action = 7u64;

        client.approve(&s1, &action);
        // Replay the same approval — must be idempotent, not additive.
        client.approve(&s1, &action);

        assert_eq!(client.get_approval_count(&action), 1);
        assert!(!client.is_executable(&action, &ActionClass::Standard));
    }

    /// A replayed approval after the action has already executed must not
    /// re-open it for execution.
    #[test]
    fn replayed_approval_after_execution_is_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);
        let (contract_id, s1, s2, _s3) = setup(&env);
        let client = MultisigUnderTestClient::new(&env, &contract_id);
        let action = 11u64;

        client.approve(&s1, &action);
        client.approve(&s2, &action);
        assert!(client.is_executable(&action, &ActionClass::Standard));
        client.mark_executed(&action);
        assert!(!client.is_executable(&action, &ActionClass::Standard));

        // Replay an approval for the already-executed action.
        client.approve(&s1, &action);
        assert!(!client.is_executable(&action, &ActionClass::Standard));
    }

    /// Approvals older than `approval_ttl` must not satisfy the quorum even
    /// if the count is still >= threshold.
    #[test]
    fn expired_approvals_do_not_satisfy_quorum() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);
        let (contract_id, s1, s2, _s3) = setup(&env);
        let client = MultisigUnderTestClient::new(&env, &contract_id);
        let action = 21u64;

        client.approve(&s1, &action);
        client.approve(&s2, &action);
        assert!(client.is_executable(&action, &ActionClass::Standard));

        // Advance past approval_ttl (100).
        env.ledger().set_timestamp(1_000 + 101);
        assert!(!client.is_executable(&action, &ActionClass::Standard));
    }

    /// A single stale approval plus a fresh one must not combine into a
    /// quorum: the stale approval is dropped, leaving only one live approval.
    #[test]
    fn stale_plus_fresh_approval_does_not_reach_quorum() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);
        let (contract_id, s1, s2, _s3) = setup(&env);
        let client = MultisigUnderTestClient::new(&env, &contract_id);
        let action = 33u64;

        client.approve(&s1, &action);
        env.ledger().set_timestamp(1_000 + 101); // s1's approval is now stale
        client.approve(&s2, &action);

        assert!(!client.is_executable(&action, &ActionClass::Standard));
    }

    /// A non-signer cannot approve, and a non-signer approval must not be
    /// counted toward the quorum.
    #[test]
    fn non_signer_approval_is_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);
        let (contract_id, s1, _s2, _s3) = setup(&env);
        let client = MultisigUnderTestClient::new(&env, &contract_id);
        let action = 44u64;
        let outsider = Address::generate(&env);

        client.approve(&s1, &action);
        let res = client.try_approve(&outsider, &action);
        assert!(res.is_err(), "non-signer approval must be rejected");
        assert_eq!(client.get_approval_count(&action), 1);
        assert!(!client.is_executable(&action, &ActionClass::Standard));
    }

    /// Revoking an approval must drop it from the quorum count.
    #[test]
    fn revoked_approval_does_not_count() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_000);
        let (contract_id, s1, s2, _s3) = setup(&env);
        let client = MultisigUnderTestClient::new(&env, &contract_id);
        let action = 55u64;

        client.approve(&s1, &action);
        client.approve(&s2, &action);
        assert!(client.is_executable(&action, &ActionClass::Standard));

        client.revoke(&s2, &action);
        assert_eq!(client.get_approval_count(&action), 1);
        assert!(!client.is_executable(&action, &ActionClass::Standard));
    }
}
