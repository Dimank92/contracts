#![no_std]

//! Shared admin/multisig/pause helpers for the Zenith contracts.
//!
//! Every admin-gated function in options_market, price_oracle and vault
//! has a `_via_multisig` twin. Both entrypoints call the same
//! `x_inner(env, Auth, ...)`, which starts with
//! `require_admin_or_multisig` — so validation lives in one place and a
//! security fix to the auth check is made once, here.
//!
//! Deliberately defines no `#[contract]` and exports no contract spec
//! entries, so linking it can't change any consumer's public ABI.
//!
//! Also home to the storage-schema migration framework (`migrations`)
//! and pinned-multisig wasm upgrades (`upgrade`) every contract uses —
//! see docs/migrations.md.

pub mod migrations;
pub mod upgrade;

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_auth;

use soroban_sdk::{
    contracttype, panic_with_error, vec, Address, BytesN, Env, IntoVal, Symbol, Val,
};

/// How sensitive an admin action is. The multisig exposes a single
/// approval tier today (`is_approved`), so the class doesn't change
/// whether an approval is accepted; it is recorded in the
/// `multisig_approval_used` event so every consumed approval can be
/// audited by tier, and it is the hook for per-class delays once the
/// multisig grows them.
#[contracttype(export = false)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ActionClass {
    /// Pause/unpause and migrations of already-installed code.
    Emergency = 0,
    /// Parameter changes and day-to-day administration.
    Standard = 1,
    /// Admin transfer, upgrades, and moving escrowed funds.
    Critical = 2,
}

/// Who is authorizing an admin action.
#[derive(Clone)]
pub enum Auth {
    /// The address stored under the contract's admin key must sign.
    Admin,
    /// `(multisig_contract, action_id)`: `multisig_contract` must be the
    /// contract's trusted multisig (`set_trusted_multisig`) and report
    /// the action as approved. Each approval authorizes exactly one call.
    Multisig(Address, BytesN<32>),
}

#[contracttype(export = false)]
#[derive(Clone)]
enum CommonKey {
    /// The only multisig `Auth::Multisig` (and pinned upgrades) accept.
    TrustedMultisig,
    /// Address proposed by `propose_admin`, waiting on `accept_admin`.
    PendingAdmin,
    /// A `(multisig, action_id)` approval this contract already acted on.
    UsedApproval(Address, BytesN<32>),
}

// About 30 days of ledgers: how long a consumed-approval marker is kept
// live. Archived (not deleted) after that, so it still blocks a replay
// until someone pays to restore it — reads of archived entries fail.
const USED_APPROVAL_TTL: u32 = 30 * 17_280;

/// Cross-calls `multisig.is_approved(action_id)`. Uses `invoke_contract`
/// directly rather than a `contractimport!` client, so no consumer needs
/// multisig's wasm to compile.
pub fn is_approved(env: &Env, multisig: &Address, action_id: &BytesN<32>) -> bool {
    env.invoke_contract(
        multisig,
        &Symbol::new(env, "is_approved"),
        vec![env, action_id.into_val(env)],
    )
}

/// True once this contract has acted on `(multisig, action_id)`.
pub fn is_approval_used(env: &Env, multisig: &Address, action_id: &BytesN<32>) -> bool {
    env.storage().persistent().has(&CommonKey::UsedApproval(
        multisig.clone(),
        action_id.clone(),
    ))
}

/// Panics with `unauthorized` unless the multisig reports `action_id`
/// approved and this contract hasn't already acted on it, then marks it
/// used. Without the marker, one approval would authorize the same (or
/// any other) admin call forever.
pub fn consume_approval<E>(
    env: &Env,
    multisig: &Address,
    action_id: &BytesN<32>,
    class: ActionClass,
    unauthorized: E,
) where
    E: Into<soroban_sdk::Error>,
{
    let key = CommonKey::UsedApproval(multisig.clone(), action_id.clone());
    if env.storage().persistent().has(&key) || !is_approved(env, multisig, action_id) {
        panic_with_error!(env, unauthorized);
    }
    env.storage().persistent().set(&key, &true);
    env.storage()
        .persistent()
        .extend_ttl(&key, USED_APPROVAL_TTL, USED_APPROVAL_TTL);
    env.events().publish(
        (
            Symbol::new(env, "multisig_approval_used"),
            multisig.clone(),
            action_id.clone(),
        ),
        class as u32,
    );
}

/// Sets the multisig every `_via_multisig` entrypoint (and pinned
/// upgrades) trusts. The caller must already have checked admin auth.
/// Without this pin, a `_via_multisig` caller could name a multisig they
/// deployed themselves (one that approves everything) and act as admin.
pub fn set_trusted_multisig(env: &Env, multisig: &Address) {
    env.storage()
        .instance()
        .set(&CommonKey::TrustedMultisig, multisig);
    env.events().publish(
        (Symbol::new(env, "trusted_multisig_set"),),
        multisig.clone(),
    );
}

pub fn trusted_multisig(env: &Env) -> Option<Address> {
    env.storage().instance().get(&CommonKey::TrustedMultisig)
}

/// Panics with `unauthorized` unless `auth` is satisfied: the stored
/// admin's signature for `Auth::Admin`, or an unused approval from the
/// trusted multisig for `Auth::Multisig`. Call it before reading or
/// writing any other state. Generic over each contract's own storage key
/// and error enum.
pub fn require_admin_or_multisig<K, E>(
    env: &Env,
    admin_key: &K,
    auth: &Auth,
    class: ActionClass,
    unauthorized: E,
) where
    K: IntoVal<Env, Val>,
    E: Copy + Into<soroban_sdk::Error>,
{
    match auth {
        Auth::Admin => get_admin_or(env, admin_key, unauthorized).require_auth(),
        Auth::Multisig(multisig, action_id) => {
            if trusted_multisig(env).as_ref() != Some(multisig) {
                panic_with_error!(env, unauthorized);
            }
            consume_approval(env, multisig, action_id, class, unauthorized)
        }
    }
}

/// The stored admin. Panics (host error) if the contract has none, i.e.
/// isn't initialized.
pub fn get_admin<K: IntoVal<Env, Val>>(env: &Env, admin_key: &K) -> Address {
    env.storage().instance().get(admin_key).unwrap()
}

fn get_admin_or<K, E>(env: &Env, admin_key: &K, err: E) -> Address
where
    K: IntoVal<Env, Val>,
    E: Into<soroban_sdk::Error>,
{
    env.storage()
        .instance()
        .get(admin_key)
        .unwrap_or_else(|| panic_with_error!(env, err))
}

/// Stores `new_admin` immediately and returns the previous admin, for the
/// caller's own `admin_transferred` event. Prefer the two-step
/// `propose_admin` / `accept_admin` for handing control to a new party.
pub fn set_admin<K: IntoVal<Env, Val>>(env: &Env, admin_key: &K, new_admin: &Address) -> Address {
    let old = get_admin(env, admin_key);
    env.storage().instance().set(admin_key, new_admin);
    env.storage().instance().remove(&CommonKey::PendingAdmin);
    old
}

/// Step one of an admin transfer: records `new_admin` as pending. The
/// caller must already have authorized the current admin. Proposing
/// again replaces the pending address; proposing the current admin
/// cancels the transfer. Nothing changes hands until `accept_admin`, so a
/// mistyped or unusable address can never strand the contract.
pub fn propose_admin<K: IntoVal<Env, Val>>(env: &Env, admin_key: &K, new_admin: &Address) {
    if *new_admin == get_admin(env, admin_key) {
        env.storage().instance().remove(&CommonKey::PendingAdmin);
    } else {
        env.storage()
            .instance()
            .set(&CommonKey::PendingAdmin, new_admin);
    }
}

pub fn pending_admin(env: &Env) -> Option<Address> {
    env.storage().instance().get(&CommonKey::PendingAdmin)
}

/// Step two: the pending admin signs to take over. Returns
/// `(old_admin, new_admin)` for the caller's `admin_transferred` event.
/// Panics with `no_pending` if nothing is pending.
pub fn accept_admin<K, E>(env: &Env, admin_key: &K, no_pending: E) -> (Address, Address)
where
    K: IntoVal<Env, Val>,
    E: Into<soroban_sdk::Error>,
{
    let pending = pending_admin(env).unwrap_or_else(|| panic_with_error!(env, no_pending));
    pending.require_auth();
    let old = set_admin(env, admin_key, &pending);
    (old, pending)
}

pub fn is_paused<K: IntoVal<Env, Val>>(env: &Env, paused_key: &K) -> bool {
    env.storage().instance().get(paused_key).unwrap_or(false)
}

pub fn set_paused<K: IntoVal<Env, Val>>(env: &Env, paused_key: &K, paused: bool) {
    env.storage().instance().set(paused_key, &paused);
}

/// Panics with `err` if the contract is paused.
pub fn require_not_paused<K, E>(env: &Env, paused_key: &K, err: E)
where
    K: IntoVal<Env, Val>,
    E: Into<soroban_sdk::Error>,
{
    if is_paused(env, paused_key) {
        panic_with_error!(env, err);
    }
}
