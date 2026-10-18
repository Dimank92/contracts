//! Wasm upgrades with a **pinned** multisig.
//!
//! `upgrade(new_wasm_hash)` needs the admin's signature. Its twin
//! `upgrade_via_multisig(action_id, new_wasm_hash)` doesn't take a
//! `multisig_contract` argument: it always uses the contract's trusted
//! multisig (`set_multisig`, see `crate::set_trusted_multisig`), so an
//! attacker can't point it at a multisig they control.
//!
//! The approved `action_id` must also equal `upgrade_action_id(hash)`,
//! derived from this contract's address and the new wasm hash. Signers
//! therefore approve one specific wasm for one specific contract, and
//! the approval can't be reused for a different hash or contract. The
//! action is `ActionClass::Critical`.

use soroban_sdk::{panic_with_error, xdr::ToXdr, Bytes, BytesN, Env, IntoVal, Symbol, Val};

use crate::{consume_approval, trusted_multisig, ActionClass};

/// Who authorizes an upgrade.
pub enum UpgradeAuth {
    /// The address stored under the contract's admin key signs.
    Admin,
    /// The trusted multisig reports `action_id` approved.
    PinnedMultisig(BytesN<32>),
}

/// The multisig `action_id` that authorizes upgrading *this* contract to
/// `new_wasm_hash`:
/// `sha256("zenith.upgrade" || xdr(this_contract) || new_wasm_hash)`.
pub fn upgrade_action_id(env: &Env, new_wasm_hash: &BytesN<32>) -> BytesN<32> {
    let mut preimage = Bytes::from_slice(env, b"zenith.upgrade");
    preimage.append(&env.current_contract_address().to_xdr(env));
    preimage.append(&Bytes::from_array(env, &new_wasm_hash.to_array()));
    env.crypto().sha256(&preimage).into()
}

/// Checks `auth`, then swaps this contract's executable for
/// `new_wasm_hash`, keeping its address and storage. The new wasm's
/// `migrate` must be called afterward if it raises the schema version.
pub fn upgrade<K, E>(
    env: &Env,
    admin_key: &K,
    auth: UpgradeAuth,
    new_wasm_hash: BytesN<32>,
    unauthorized: E,
) where
    K: IntoVal<Env, Val>,
    E: Copy + Into<soroban_sdk::Error>,
{
    match auth {
        UpgradeAuth::Admin => crate::get_admin(env, admin_key).require_auth(),
        UpgradeAuth::PinnedMultisig(action_id) => {
            let multisig =
                trusted_multisig(env).unwrap_or_else(|| panic_with_error!(env, unauthorized));
            if action_id != upgrade_action_id(env, &new_wasm_hash) {
                panic_with_error!(env, unauthorized);
            }
            consume_approval(
                env,
                &multisig,
                &action_id,
                ActionClass::Critical,
                unauthorized,
            );
        }
    }
    env.deployer()
        .update_current_contract_wasm(new_wasm_hash.clone());
    env.events()
        .publish((Symbol::new(env, "upgraded"),), new_wasm_hash);
}
