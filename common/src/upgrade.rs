//! Wasm upgrades with a **pinned** multisig.
//!
//! `upgrade(new_wasm_hash)` needs the admin's signature. Its twin
//! `upgrade_via_multisig(action_id, new_wasm_hash)` doesn't take a
//! `multisig_contract` argument the way the older `_via_multisig`
//! entrypoints do (docs/governance-handover.md, "Known caveat"): it only
//! trusts the multisig the admin pinned with `set_upgrade_multisig`, so an
//! attacker can't point it at a multisig they control.
//!
//! The approved `action_id` must also equal `upgrade_action_id(hash)`,
//! derived from this contract's address and the new wasm hash. Signers
//! therefore approve one specific wasm for one specific contract, and
//! the approval can't be reused for a different hash or contract. The
//! action is `ActionClass::Critical`, so the multisig's longest
//! timelock applies.

use soroban_sdk::{
    contracttype, panic_with_error, xdr::ToXdr, Address, Bytes, BytesN, Env, IntoVal, Symbol, Val,
};

use crate::{is_executable, ActionClass};

#[contracttype(export = false)]
#[derive(Clone)]
enum UpgradeKey {
    PinnedMultisig,
}

/// Who authorizes an upgrade.
pub enum UpgradeAuth {
    /// The address stored under the contract's admin key signs.
    Admin,
    /// The pinned multisig reports `action_id` executable as `Critical`.
    PinnedMultisig(u64),
}

/// Pins (or re-pins) the multisig `upgrade_via_multisig` trusts. The
/// caller must already have checked admin auth.
pub fn pin_multisig(env: &Env, multisig: &Address) {
    env.storage()
        .instance()
        .set(&UpgradeKey::PinnedMultisig, multisig);
    env.events()
        .publish((Symbol::new(env, "upgrade_multisig_pinned"),), multisig.clone());
}

/// Multisig pinned for wasm upgrades, if any. Read-only.
pub fn pinned_multisig(env: &Env) -> Option<Address> {
    env.storage().instance().get(&UpgradeKey::PinnedMultisig)
}

/// The multisig `action_id` that authorizes upgrading *this* contract to
/// `new_wasm_hash`: the first 8 bytes (big-endian) of
/// `sha256("zenith.upgrade" || xdr(this_contract) || new_wasm_hash)`.
pub fn upgrade_action_id(env: &Env, new_wasm_hash: &BytesN<32>) -> u64 {
    let mut preimage = Bytes::from_slice(env, b"zenith.upgrade");
    preimage.append(&env.current_contract_address().to_xdr(env));
    preimage.append(&Bytes::from_array(env, &new_wasm_hash.to_array()));
    let digest = env.crypto().sha256(&preimage).to_array();
    let mut id = [0u8; 8];
    id.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(id)
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
                pinned_multisig(env).unwrap_or_else(|| panic_with_error!(env, unauthorized));
            if action_id != upgrade_action_id(env, &new_wasm_hash)
                || !is_executable(env, &multisig, action_id, ActionClass::Critical)
            {
                panic_with_error!(env, unauthorized);
            }
        }
    }
    env.deployer()
        .update_current_contract_wasm(new_wasm_hash.clone());
    env.events()
        .publish((Symbol::new(env, "upgraded"),), new_wasm_hash);
}
