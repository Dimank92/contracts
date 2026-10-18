//! Adversarial vault contracts.
//!
//! Each adversary here exposes the vault interface that options_market calls
//! via `contractimport!` — specifically `deposit`, `withdraw`, and
//! `balance_of`, plus the `Tag` type used to namespace escrow.
//!
//! ## Adversaries
//!
//! | Struct               | Behaviour                                                        |
//! |----------------------|------------------------------------------------------------------|
//! | [`NoOpVault`]        | Every mutating call (`deposit`, `withdraw`) silently succeeds without moving tokens. |
//! | [`WrongBalanceVault`] | `balance_of()` returns a value that doesn't match the real token balance. |
//! | [`RevertOnWithdrawVault`] | `withdraw()` always panics — funds can be deposited but never retrieved. |
//! | [`DrainVault`]       | `withdraw()` allows withdrawing more than was deposited, modelling an accounting underflow bug. |

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Env, Symbol,
};

// ─── shared Tag type (mirrors vault's Tag) ───────────────────────────────────

/// Mirrors `vault::Tag` so that options_market's `series_tag` / `position_tag`
/// helpers produce values that match what these adversarial vaults expect.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tag {
    pub owner: Address,
    pub kind: Symbol,
    pub id: u64,
}

#[contracttype]
enum VaultKey {
    Admin,
    Escrow(Address, Tag), // (token, tag) → balance
    Total(Address),       // token → total escrowed
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum VaultError {
    WithdrawReverted = 1,
    InsufficientEscrowBalance = 3,
}

// ─── NoOpVault ────────────────────────────────────────────────────────────────

pub use no_op_vault::{NoOpVault, NoOpVaultClient};

mod no_op_vault {
    use super::*;

    /// A vault whose `deposit` and `withdraw` both silently no-op. No tokens
    /// ever move; no escrow ledger is updated.
    ///
    /// **Vulnerability exercised (issue #115):** `options_market::escrow_series_to_vault`
    /// moves the series' liability to the vault, then subsequent
    /// `claim_refund_from_vault` calls try to `withdraw` from it.
    /// If the vault is a no-op, `claim_refund_from_vault` will receive 0 tokens
    /// even though the options_market thinks the refund was paid. Tests confirm
    /// options_market either checks the vault's return value or the token
    /// balance delta.
    #[contract]
    pub struct NoOpVault;

    #[contractimpl]
    impl NoOpVault {
        pub fn initialize(env: Env, admin: Address, _token: Address, _max_pause_duration: u64) {
            admin.require_auth();
            env.storage().instance().set(&VaultKey::Admin, &admin);
        }

        /// No-op: does NOT credit any escrow, does NOT pull any tokens.
        /// Returns 0 (the measured delta is always zero here).
        pub fn deposit(
            _env: Env,
            _from: Address,
            _token: Address,
            _tag: Tag,
            _amount: i128,
        ) -> i128 {
            0
        }

        /// No-op: does NOT push any tokens to `to`.
        pub fn withdraw(_env: Env, _token: Address, _tag: Tag, _to: Address, _amount: i128) {}

        pub fn balance_of(_env: Env, _token: Address, _tag: Tag) -> i128 {
            0
        }
        pub fn get_total_escrowed(_env: Env, _token: Address) -> i128 {
            0
        }
        pub fn is_paused(_env: Env) -> bool {
            false
        }
        pub fn get_admin(env: Env) -> Address {
            env.storage().instance().get(&VaultKey::Admin).unwrap()
        }
        pub fn is_token_allowed(_env: Env, _token: Address) -> bool {
            true
        }
        pub fn is_integrator(_env: Env, _addr: Address) -> bool {
            true
        }
        pub fn set_token_allowed(_env: Env, _token: Address, _allowed: bool) {}
        pub fn set_integrator(_env: Env, _integrator: Address, _allowed: bool) {}
        pub fn transfer_tag(_env: Env, _token: Address, _from: Tag, _to: Tag, _amount: i128) {}
        pub fn get_tag_owner(_env: Env, _tag: Tag) -> Option<Address> {
            None
        }

        // Legacy wrappers
        pub fn deposit_legacy(_env: Env, _from: Address, _id: u64, _amount: i128) -> i128 {
            0
        }
        pub fn withdraw_legacy(_env: Env, _id: u64, _to: Address, _amount: i128) {}
        pub fn balance_of_legacy(_env: Env, _id: u64) -> i128 {
            0
        }
        pub fn legacy_tag(env: Env, id: u64) -> Tag {
            Tag {
                owner: env.storage().instance().get(&VaultKey::Admin).unwrap(),
                kind: symbol_short!("legacy"),
                id,
            }
        }
    }
}

// ─── WrongBalanceVault ────────────────────────────────────────────────────────

pub use wrong_balance_vault::{WrongBalanceVault, WrongBalanceVaultClient};

mod wrong_balance_vault {
    use super::*;

    /// A vault that correctly records deposits but reports wildly incorrect
    /// balances from `balance_of`: it always returns twice the true balance.
    ///
    /// **Vulnerability exercised (issue #115):** If options_market reads
    /// `vault.balance_of(token, tag)` and trusts it to authorise a withdrawal,
    /// an inflated balance could allow withdrawing more than was deposited.
    /// Tests confirm options_market caps withdrawals to the actual escrow amount.
    #[contract]
    pub struct WrongBalanceVault;

    #[contractimpl]
    impl WrongBalanceVault {
        pub fn initialize(env: Env, admin: Address, _token: Address, _max_pause_duration: u64) {
            admin.require_auth();
            env.storage().instance().set(&VaultKey::Admin, &admin);
        }

        pub fn deposit(env: Env, from: Address, token: Address, tag: Tag, amount: i128) -> i128 {
            from.require_auth();
            let key = VaultKey::Escrow(token.clone(), tag.clone());
            let current: i128 = env.storage().instance().get(&key).unwrap_or(0);
            env.storage().instance().set(&key, &(current + amount));
            let total_key = VaultKey::Total(token.clone());
            let total: i128 = env.storage().instance().get(&total_key).unwrap_or(0);
            env.storage().instance().set(&total_key, &(total + amount));
            amount // pretend full amount was credited
        }

        pub fn withdraw(env: Env, token: Address, tag: Tag, to: Address, amount: i128) {
            to.require_auth();
            let key = VaultKey::Escrow(token.clone(), tag.clone());
            let current: i128 = env.storage().instance().get(&key).unwrap_or(0);
            if current < amount {
                panic_with_error!(env, VaultError::InsufficientEscrowBalance);
            }
            env.storage().instance().set(&key, &(current - amount));
        }

        /// Returns 2× the real balance — the lie.
        pub fn balance_of(env: Env, token: Address, tag: Tag) -> i128 {
            let key = VaultKey::Escrow(token, tag);
            let real: i128 = env.storage().instance().get(&key).unwrap_or(0);
            real.saturating_mul(2)
        }

        pub fn get_total_escrowed(env: Env, token: Address) -> i128 {
            env.storage()
                .instance()
                .get(&VaultKey::Total(token))
                .unwrap_or(0)
        }

        pub fn is_paused(_env: Env) -> bool {
            false
        }
        pub fn get_admin(env: Env) -> Address {
            env.storage().instance().get(&VaultKey::Admin).unwrap()
        }
        pub fn is_token_allowed(_env: Env, _token: Address) -> bool {
            true
        }
        pub fn is_integrator(_env: Env, _addr: Address) -> bool {
            true
        }
        pub fn set_token_allowed(_env: Env, _token: Address, _allowed: bool) {}
        pub fn set_integrator(_env: Env, _integrator: Address, _allowed: bool) {}
        pub fn transfer_tag(_env: Env, _token: Address, _from: Tag, _to: Tag, _amount: i128) {}
        pub fn get_tag_owner(_env: Env, _tag: Tag) -> Option<Address> {
            None
        }
        pub fn deposit_legacy(_env: Env, _from: Address, _id: u64, _amount: i128) -> i128 {
            0
        }
        pub fn withdraw_legacy(_env: Env, _id: u64, _to: Address, _amount: i128) {}
        pub fn balance_of_legacy(_env: Env, _id: u64) -> i128 {
            0
        }
        pub fn legacy_tag(env: Env, id: u64) -> Tag {
            Tag {
                owner: env.storage().instance().get(&VaultKey::Admin).unwrap(),
                kind: symbol_short!("legacy"),
                id,
            }
        }
    }
}

// ─── RevertOnWithdrawVault ────────────────────────────────────────────────────

pub use revert_on_withdraw_vault::{RevertOnWithdrawVault, RevertOnWithdrawVaultClient};

mod revert_on_withdraw_vault {
    use super::*;

    /// A vault whose `deposit` works normally but `withdraw` always panics.
    /// Models a vault that accepts funds but cannot pay them out (a "hotel
    /// california" / locked-funds scenario).
    ///
    /// **Vulnerability exercised (issue #115):** `claim_refund_from_vault` calls
    /// `vault.withdraw`; if that panics the whole tx reverts and the position
    /// holder is stuck. Tests confirm the error propagates and options_market
    /// does not mark the position as refunded unless the transfer succeeds.
    #[contract]
    pub struct RevertOnWithdrawVault;

    #[contractimpl]
    impl RevertOnWithdrawVault {
        pub fn initialize(env: Env, admin: Address, _token: Address, _max_pause_duration: u64) {
            admin.require_auth();
            env.storage().instance().set(&VaultKey::Admin, &admin);
        }

        pub fn deposit(env: Env, from: Address, token: Address, tag: Tag, amount: i128) -> i128 {
            from.require_auth();
            let key = VaultKey::Escrow(token.clone(), tag.clone());
            let current: i128 = env.storage().instance().get(&key).unwrap_or(0);
            env.storage().instance().set(&key, &(current + amount));
            amount
        }

        /// Always panics — funds are locked forever.
        pub fn withdraw(env: Env, _token: Address, _tag: Tag, _to: Address, _amount: i128) {
            panic_with_error!(env, VaultError::WithdrawReverted);
        }

        pub fn balance_of(env: Env, token: Address, tag: Tag) -> i128 {
            env.storage()
                .instance()
                .get(&VaultKey::Escrow(token, tag))
                .unwrap_or(0)
        }

        pub fn get_total_escrowed(_env: Env, _token: Address) -> i128 {
            0
        }
        pub fn is_paused(_env: Env) -> bool {
            false
        }
        pub fn get_admin(env: Env) -> Address {
            env.storage().instance().get(&VaultKey::Admin).unwrap()
        }
        pub fn is_token_allowed(_env: Env, _token: Address) -> bool {
            true
        }
        pub fn is_integrator(_env: Env, _addr: Address) -> bool {
            true
        }
        pub fn set_token_allowed(_env: Env, _token: Address, _allowed: bool) {}
        pub fn set_integrator(_env: Env, _integrator: Address, _allowed: bool) {}
        pub fn transfer_tag(_env: Env, _token: Address, _from: Tag, _to: Tag, _amount: i128) {}
        pub fn get_tag_owner(_env: Env, _tag: Tag) -> Option<Address> {
            None
        }
        pub fn deposit_legacy(_env: Env, _from: Address, _id: u64, _amount: i128) -> i128 {
            0
        }
        pub fn withdraw_legacy(_env: Env, _id: u64, _to: Address, _amount: i128) {}
        pub fn balance_of_legacy(_env: Env, _id: u64) -> i128 {
            0
        }
        pub fn legacy_tag(env: Env, id: u64) -> Tag {
            Tag {
                owner: env.storage().instance().get(&VaultKey::Admin).unwrap(),
                kind: symbol_short!("legacy"),
                id,
            }
        }
    }
}

// ─── DrainVault ──────────────────────────────────────────────────────────────

pub use drain_vault::{DrainVault, DrainVaultClient};

mod drain_vault {
    use super::*;

    /// A vault with an accounting bug: `withdraw` allows the caller to withdraw
    /// more than was deposited under a tag without enforcing the escrow cap.
    /// The internal ledger goes negative but the call succeeds.
    ///
    /// **Vulnerability exercised (issue #115):** The real vault enforces
    /// `InsufficientEscrowBalance` — this adversary skips that check. Any
    /// code that calls `withdraw` on a vault it doesn't own should get the
    /// escrow balance from `balance_of` first; this adversary models the case
    /// where the vault itself can't be trusted to enforce the cap.
    ///
    /// Also models a vault with a shortfall: `get_total_escrowed` can exceed
    /// the vault's real token balance after an overdrain.
    #[contract]
    pub struct DrainVault;

    #[contractimpl]
    impl DrainVault {
        pub fn initialize(env: Env, admin: Address, _token: Address, _max_pause_duration: u64) {
            admin.require_auth();
            env.storage().instance().set(&VaultKey::Admin, &admin);
        }

        pub fn deposit(env: Env, from: Address, token: Address, tag: Tag, amount: i128) -> i128 {
            from.require_auth();
            let key = VaultKey::Escrow(token.clone(), tag.clone());
            let current: i128 = env.storage().instance().get(&key).unwrap_or(0);
            env.storage().instance().set(&key, &(current + amount));
            let total_key = VaultKey::Total(token.clone());
            let total: i128 = env.storage().instance().get(&total_key).unwrap_or(0);
            env.storage().instance().set(&total_key, &(total + amount));
            amount
        }

        /// No escrow-cap check: sets the ledger negative if `amount > balance`.
        pub fn withdraw(env: Env, token: Address, tag: Tag, _to: Address, amount: i128) {
            // no auth check — the drain vulnerability
            let key = VaultKey::Escrow(token.clone(), tag.clone());
            let current: i128 = env.storage().instance().get(&key).unwrap_or(0);
            // deliberately allows negative escrow
            env.storage().instance().set(&key, &(current - amount));
            let total_key = VaultKey::Total(token.clone());
            let total: i128 = env.storage().instance().get(&total_key).unwrap_or(0);
            env.storage().instance().set(&total_key, &(total - amount));
        }

        pub fn balance_of(env: Env, token: Address, tag: Tag) -> i128 {
            env.storage()
                .instance()
                .get(&VaultKey::Escrow(token, tag))
                .unwrap_or(0)
        }

        pub fn get_total_escrowed(env: Env, token: Address) -> i128 {
            env.storage()
                .instance()
                .get(&VaultKey::Total(token))
                .unwrap_or(0)
        }

        pub fn is_paused(_env: Env) -> bool {
            false
        }
        pub fn get_admin(env: Env) -> Address {
            env.storage().instance().get(&VaultKey::Admin).unwrap()
        }
        pub fn is_token_allowed(_env: Env, _token: Address) -> bool {
            true
        }
        pub fn is_integrator(_env: Env, _addr: Address) -> bool {
            true
        }
        pub fn set_token_allowed(_env: Env, _token: Address, _allowed: bool) {}
        pub fn set_integrator(_env: Env, _integrator: Address, _allowed: bool) {}
        pub fn transfer_tag(_env: Env, _token: Address, _from: Tag, _to: Tag, _amount: i128) {}
        pub fn get_tag_owner(_env: Env, _tag: Tag) -> Option<Address> {
            None
        }
        pub fn deposit_legacy(_env: Env, _from: Address, _id: u64, _amount: i128) -> i128 {
            0
        }
        pub fn withdraw_legacy(_env: Env, _id: u64, _to: Address, _amount: i128) {}
        pub fn balance_of_legacy(_env: Env, _id: u64) -> i128 {
            0
        }
        pub fn legacy_tag(env: Env, id: u64) -> Tag {
            Tag {
                owner: env.storage().instance().get(&VaultKey::Admin).unwrap(),
                kind: symbol_short!("legacy"),
                id,
            }
        }
    }
}
