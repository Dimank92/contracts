// Public API docs are enforced by `cargo xtask doc` (#268): the gate builds
// rustdoc, where this lint is on, and only there — soroban-sdk macro output is
// undocumented, which would otherwise fail `clippy -D warnings`.
#![cfg_attr(doc, warn(missing_docs))]
#![no_std]

//! Shared admin/multisig/pause helpers for the Zenith contracts.
//!
//! Every admin-gated function in options_market, price_oracle and vault
//! has a `_via_multisig` twin. Both entrypoints now call the same
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

use soroban_sdk::{contracttype, panic_with_error, vec, Address, Env, IntoVal, Symbol, Val};

/// Mirrors multisig's `ActionClass` (same `u32` encoding) without
/// exporting a spec entry into consumer contracts.
#[contracttype(export = false)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ActionClass {
    /// Time-critical response: pausing, vetoes and similar safety actions.
    Emergency = 0,
    /// Routine administration: parameter and configuration changes.
    Standard = 1,
    /// High-impact changes: upgrades, signer rotation, admin handover.
    Critical = 2,
}

/// Who is authorizing an admin action.
pub enum Auth {
    /// The address stored under the contract's admin key must sign.
    Admin,
    /// `(multisig_contract, action_id)`: the multisig must report the
    /// action as executable for the entrypoint's `ActionClass`.
    Multisig(Address, u64),
}

/// Cross-calls `multisig.is_executable(action_id, class)`. Uses
/// `invoke_contract` directly rather than a `contractimport!` client, so
/// no consumer needs multisig's wasm to compile.
pub fn is_executable(env: &Env, multisig: &Address, action_id: u64, class: ActionClass) -> bool {
    env.invoke_contract(
        multisig,
        &Symbol::new(env, "is_executable"),
        vec![env, action_id.into_val(env), class.into_val(env)],
    )
}

/// Panics with `unauthorized` unless `auth` is satisfied: the stored
/// admin's signature for `Auth::Admin`, or an executable multisig
/// approval for `Auth::Multisig`. Generic over each contract's own
/// storage key and error enum.
pub fn require_admin_or_multisig<K, E>(
    env: &Env,
    admin_key: &K,
    auth: &Auth,
    class: ActionClass,
    unauthorized: E,
) where
    K: IntoVal<Env, Val>,
    E: Into<soroban_sdk::Error>,
{
    match auth {
        Auth::Admin => get_admin(env, admin_key).require_auth(),
        Auth::Multisig(multisig, action_id) => {
            if !is_executable(env, multisig, *action_id, class) {
                panic_with_error!(env, unauthorized);
            }
        }
    }
}

/// Contract error code every Zenith contract reserves for
/// `Error::InternalInvariant`, raised when a guaranteed-present value is
/// missing instead of panicking.
pub const INTERNAL_INVARIANT: u32 = 90;

/// Returns the stored admin address. Fails with `InternalInvariant` (90) if the contract was never initialized.
pub fn get_admin<K: IntoVal<Env, Val>>(env: &Env, admin_key: &K) -> Address {
    env.storage().instance().get(admin_key).unwrap_or_else(|| {
        panic_with_error!(
            env,
            soroban_sdk::Error::from_contract_error(INTERNAL_INVARIANT)
        )
    })
}

/// Stores `new_admin` and returns the previous admin, for the caller's
/// own `admin_transferred` event.
pub fn set_admin<K: IntoVal<Env, Val>>(env: &Env, admin_key: &K, new_admin: &Address) -> Address {
    let old = get_admin(env, admin_key);
    env.storage().instance().set(admin_key, new_admin);
    old
}

/// Whether the contract is paused under `paused_key`. Read-only.
pub fn is_paused<K: IntoVal<Env, Val>>(env: &Env, paused_key: &K) -> bool {
    env.storage().instance().get(paused_key).unwrap_or(false)
}

/// Sets the paused flag under `paused_key`. Callers must authorize first.
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
