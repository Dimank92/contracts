//! Adversarial multisig contracts.
//!
//! Each adversary here exposes `is_approved(action_id) -> bool` — the only
//! function `zenith-common`'s `require_admin_or_multisig` ever calls on a
//! multisig via `invoke_contract`. No wasm is needed because common calls
//! it with `invoke_contract` by symbol, not via `contractimport!`.
//!
//! ## Adversaries
//!
//! | Struct                     | Behaviour                                                    |
//! |----------------------------|--------------------------------------------------------------|
//! | [`AlwaysApproveMultisig`]  | `is_approved()` always returns `true` — any action is immediately approved with no quorum. |
//! | [`NeverApproveMultisig`]   | `is_approved()` always returns `false` — no action can ever proceed. |
//! | [`SingleSignerMultisig`]   | Only one signer and threshold 1; the weakest legitimate multisig. |
//! | [`ExpiredApprovalMultisig`] | Records approvals but reports every one as expired. |

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Vec};

pub use soroban_sdk::BytesN;

/// Convenience: build a 32-byte action id from a `u64` discriminant.
/// The bytes are the little-endian u64 followed by 24 zero bytes.
pub fn action_id_from_u64(env: &Env, discriminant: u64) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&discriminant.to_le_bytes());
    BytesN::from_array(env, &bytes)
}

#[contracttype]
enum MultisigKey {
    Signers,
}

// ─── AlwaysApproveMultisig ───────────────────────────────────────────────────

pub use always_approve_multisig::{AlwaysApproveMultisig, AlwaysApproveMultisigClient};

mod always_approve_multisig {
    use super::*;

    /// A multisig whose `is_approved` always returns `true` regardless of
    /// `action_id` or how many approvals exist. Anyone can invoke any
    /// `_via_multisig` function on any contract that trusts this address
    /// as its multisig.
    ///
    /// **Vulnerability exercised (issue #115):** the `_via_multisig`
    /// entrypoints that take the multisig address as an argument trust
    /// whatever address the caller passes. Tests pin that down, and check
    /// that pinned-multisig paths (`upgrade_via_multisig`) reject it.
    #[contract]
    pub struct AlwaysApproveMultisig;

    #[contractimpl]
    impl AlwaysApproveMultisig {
        pub fn initialize(env: Env, signers: Vec<Address>, _threshold: u32, _approval_ttl: u64) {
            env.storage()
                .instance()
                .set(&MultisigKey::Signers, &signers);
        }

        /// Always returns `true`.
        pub fn is_approved(_env: Env, _action_id: BytesN<32>) -> bool {
            true
        }

        pub fn approve(_env: Env, _signer: Address, _action_id: BytesN<32>) {}
        pub fn revoke(_env: Env, _signer: Address, _action_id: BytesN<32>) {}
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
        pub fn has_approved(_env: Env, _action_id: BytesN<32>, _signer: Address) -> bool {
            true
        }
        pub fn get_approval_count(_env: Env, _action_id: BytesN<32>) -> u32 {
            u32::MAX
        }
    }
}

// ─── NeverApproveMultisig ────────────────────────────────────────────────────

pub use never_approve_multisig::{NeverApproveMultisig, NeverApproveMultisigClient};

mod never_approve_multisig {
    use super::*;

    /// A multisig whose `is_approved` always returns `false`. Any
    /// `_via_multisig` call that relies on it reverts with `Unauthorized`.
    #[contract]
    pub struct NeverApproveMultisig;

    #[contractimpl]
    impl NeverApproveMultisig {
        pub fn initialize(env: Env, signers: Vec<Address>, _threshold: u32, _approval_ttl: u64) {
            env.storage()
                .instance()
                .set(&MultisigKey::Signers, &signers);
        }

        /// Always returns `false`.
        pub fn is_approved(_env: Env, _action_id: BytesN<32>) -> bool {
            false
        }

        pub fn approve(_env: Env, _signer: Address, _action_id: BytesN<32>) {}
        pub fn revoke(_env: Env, _signer: Address, _action_id: BytesN<32>) {}
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
        pub fn has_approved(_env: Env, _action_id: BytesN<32>, _signer: Address) -> bool {
            false
        }
        pub fn get_approval_count(_env: Env, _action_id: BytesN<32>) -> u32 {
            0
        }
    }
}

// ─── SingleSignerMultisig ────────────────────────────────────────────────────

pub use single_signer_multisig::{SingleSignerMultisig, SingleSignerMultisigClient};

mod single_signer_multisig {
    use super::*;

    /// The weakest *legitimate* multisig: 1-of-1. Useful for positive-path
    /// tests where the multisig path must succeed without giving every
    /// caller blanket approval.
    ///
    /// `set_action_approved(action_id)` manually marks an action approved
    /// for tests that need fine-grained control.
    #[contract]
    pub struct SingleSignerMultisig;

    #[contracttype]
    enum SingleSignerKey {
        Signer,
        Approved(BytesN<32>),
    }

    fn approved(env: &Env, action_id: BytesN<32>) -> bool {
        env.storage()
            .instance()
            .get(&SingleSignerKey::Approved(action_id))
            .unwrap_or(false)
    }

    #[contractimpl]
    impl SingleSignerMultisig {
        pub fn initialize(env: Env, signer: Address, _approval_ttl: u64) {
            env.storage()
                .instance()
                .set(&SingleSignerKey::Signer, &signer);
        }

        /// Manually mark `action_id` as approved for use in tests.
        pub fn set_action_approved(env: Env, action_id: BytesN<32>, approved: bool) {
            env.storage()
                .instance()
                .set(&SingleSignerKey::Approved(action_id), &approved);
        }

        pub fn is_approved(env: Env, action_id: BytesN<32>) -> bool {
            approved(&env, action_id)
        }

        pub fn approve(env: Env, signer: Address, action_id: BytesN<32>) {
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

        pub fn revoke(env: Env, _signer: Address, action_id: BytesN<32>) {
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

        pub fn has_approved(env: Env, action_id: BytesN<32>, _signer: Address) -> bool {
            approved(&env, action_id)
        }

        pub fn get_approval_count(env: Env, action_id: BytesN<32>) -> u32 {
            u32::from(approved(&env, action_id))
        }
    }
}

// ─── ExpiredApprovalMultisig ─────────────────────────────────────────────────

pub use expired_approval_multisig::{ExpiredApprovalMultisig, ExpiredApprovalMultisigClient};

mod expired_approval_multisig {
    use super::*;

    /// A multisig that records approvals but `is_approved` always returns
    /// `false` because every approval is treated as expired (approval_ttl
    /// = 1 second and the ledger has already advanced past it).
    ///
    /// **Vulnerability exercised (issue #115):** `_via_multisig` calls
    /// must fail even if quorum was previously reached, once the approval
    /// expired. Confirms the consumers don't cache `is_approved` across
    /// ledgers.
    #[contract]
    pub struct ExpiredApprovalMultisig;

    #[contracttype]
    enum ExpiredKey {
        ApprovalTime(BytesN<32>),
    }

    #[contractimpl]
    impl ExpiredApprovalMultisig {
        pub fn initialize(env: Env, signers: Vec<Address>, _threshold: u32) {
            env.storage()
                .instance()
                .set(&MultisigKey::Signers, &signers);
        }

        /// Record an approval timestamp so tests can set up state.
        pub fn record_approval(env: Env, action_id: BytesN<32>, timestamp: u64) {
            env.storage()
                .instance()
                .set(&ExpiredKey::ApprovalTime(action_id), &timestamp);
        }

        /// Always `false` — the approval is always expired.
        pub fn is_approved(_env: Env, _action_id: BytesN<32>) -> bool {
            false
        }

        pub fn approve(env: Env, _signer: Address, action_id: BytesN<32>) {
            env.storage().instance().set(
                &ExpiredKey::ApprovalTime(action_id),
                &env.ledger().timestamp(),
            );
        }
        pub fn revoke(_env: Env, _signer: Address, _action_id: BytesN<32>) {}
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
        }
        pub fn has_approved(_env: Env, _action_id: BytesN<32>, _signer: Address) -> bool {
            false
        }
        pub fn get_approval_count(_env: Env, _action_id: BytesN<32>) -> u32 {
            0
        }
    }
}
