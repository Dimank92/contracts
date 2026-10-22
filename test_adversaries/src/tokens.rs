//! Adversarial token contracts.
//!
//! Each adversary here implements the minimal SEP-41 / Soroban token
//! interface (`transfer`, `balance`, `approve`, `allowance`, `mint`,
//! `burn`, `decimals`, `name`, `symbol`) so that any code that calls
//! `soroban_sdk::token::Client` against them will compile and run.
//!
//! ## Adversaries
//!
//! | Struct                  | Behaviour                                                    |
//! |-------------------------|--------------------------------------------------------------|
//! | [`LyingBalanceToken`]   | `balance()` always returns `i128::MAX` regardless of state. |
//! | [`FeeOnTransferToken`]  | Silently deducts a 10% fee on every `transfer`, delivering only 90% of the requested amount. |
//! | [`RevertOnTransferToken`] | `transfer()` always panics — every inbound transfer attempt fails. |
//! | [`ReentrantToken`]      | On `transfer`, tries to call back into the *caller* contract before completing, exercising reentrancy guards. |
//! | [`ZeroBalanceToken`]    | `balance()` always returns 0 even after transfers.           |
//! | [`OverflowToken`]       | `balance()` returns `i128::MAX`; `transfer()` tries to move `i128::MAX` as the amount to trigger overflow paths. |

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Env, String, Val,
};

// ─── shared storage helpers ───────────────────────────────────────────────────

#[contracttype]
enum TokenKey {
    Balance(Address),
    Allowance(Address, Address),
}

fn get_balance(env: &Env, account: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&TokenKey::Balance(account.clone()))
        .unwrap_or(0i128)
}

fn set_balance(env: &Env, account: &Address, amount: i128) {
    env.storage()
        .instance()
        .set(&TokenKey::Balance(account.clone()), &amount);
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum TokenError {
    InsufficientBalance = 1,
    TransferReverted = 2,
}

// ─── LyingBalanceToken ───────────────────────────────────────────────────────

pub mod lying_balance_token {
    use super::*;
    /// A token whose `balance()` always reports `i128::MAX` regardless of the
    /// actual on-chain state. Exercises any code that trusts the reported
    /// balance without verifying real transfers (e.g. a vault that accepts a
    /// token deposit and records `amount` without measuring the actual delta).
    ///
    /// **Vulnerability exercised (issue #115):** If the vault's `deposit`
    /// records the caller-supplied `amount` rather than the measured balance
    /// delta, a lying-balance token lets an attacker inflate their escrow
    /// credit for free.
    #[contract]
    pub struct LyingBalanceToken;

    #[contractimpl]
    impl LyingBalanceToken {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            set_balance(&env, &admin, 1_000_000_000_000);
        }

        pub fn mint(env: Env, to: Address, amount: i128) {
            let bal = get_balance(&env, &to);
            set_balance(&env, &to, bal + amount);
        }

        pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
            from.require_auth();
            let from_bal = get_balance(&env, &from);
            let to_bal = get_balance(&env, &to);
            set_balance(&env, &from, from_bal - amount);
            set_balance(&env, &to, to_bal + amount);
        }

        /// Always returns `i128::MAX` — the lie.
        pub fn balance(_env: Env, _id: Address) -> i128 {
            i128::MAX
        }

        pub fn approve(
            env: Env,
            from: Address,
            spender: Address,
            amount: i128,
            _expiration_ledger: u32,
        ) {
            from.require_auth();
            env.storage()
                .instance()
                .set(&TokenKey::Allowance(from, spender), &amount);
        }

        pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
            env.storage()
                .instance()
                .get(&TokenKey::Allowance(from, spender))
                .unwrap_or(0i128)
        }

        pub fn decimals(_env: Env) -> u32 {
            7
        }
        pub fn name(env: Env) -> String {
            String::from_str(&env, "LyingBalanceToken")
        }
        pub fn symbol(env: Env) -> String {
            String::from_str(&env, "LIE")
        }
        pub fn burn(_env: Env, _from: Address, _amount: i128) {}
        pub fn burn_from(_env: Env, _spender: Address, _from: Address, _amount: i128) {}
        pub fn transfer_from(
            _env: Env,
            _spender: Address,
            _from: Address,
            _to: Address,
            _amount: i128,
        ) {
        }
        pub fn set_authorized(_env: Env, _id: Address, _authorize: bool) {}
        pub fn authorized(_env: Env, _id: Address) -> bool {
            true
        }
    }
}
pub use lying_balance_token::{LyingBalanceToken, LyingBalanceTokenClient};

// ─── FeeOnTransferToken ───────────────────────────────────────────────────────

pub mod fee_on_transfer_token {
    use super::*;
    /// A token that silently deducts a 10% fee on every `transfer`, crediting
    /// only 90% of the requested `amount` to the recipient. The sender is
    /// still debited the full `amount`.
    ///
    /// **Vulnerability exercised (issue #115):** The vault's `deposit` checks
    /// the actual balance delta and credits only what arrived (`safe_inbound_transfer`).
    /// Any code that instead credits `amount` will over-credit the depositor by ~11%.
    /// Negative tests confirm the vault's delta-check path is taken.
    #[contract]
    pub struct FeeOnTransferToken;

    /// Fee numerator: 10% fee → recipient gets 90%.
    const FEE_BPS: i128 = 1_000; // 1 000 bps = 10%

    #[contractimpl]
    impl FeeOnTransferToken {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            set_balance(&env, &admin, 1_000_000_000_000);
        }

        pub fn mint(env: Env, to: Address, amount: i128) {
            let bal = get_balance(&env, &to);
            set_balance(&env, &to, bal + amount);
        }

        /// Delivers only 90% of `amount` to `to`; burns the remaining 10%.
        pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
            from.require_auth();
            let fee = (amount * FEE_BPS) / 10_000;
            let delivered = amount - fee;
            let from_bal = get_balance(&env, &from);
            let to_bal = get_balance(&env, &to);
            set_balance(&env, &from, from_bal - amount);
            set_balance(&env, &to, to_bal + delivered);
            // fee is burned (neither credited nor stored)
            env.events()
                .publish((symbol_short!("fee_burn"),), (from.clone(), amount, fee));
        }

        pub fn balance(env: Env, id: Address) -> i128 {
            get_balance(&env, &id)
        }

        pub fn approve(
            env: Env,
            from: Address,
            spender: Address,
            amount: i128,
            _expiration_ledger: u32,
        ) {
            from.require_auth();
            env.storage()
                .instance()
                .set(&TokenKey::Allowance(from, spender), &amount);
        }

        pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
            env.storage()
                .instance()
                .get(&TokenKey::Allowance(from, spender))
                .unwrap_or(0i128)
        }

        pub fn decimals(_env: Env) -> u32 {
            7
        }
        pub fn name(env: Env) -> String {
            String::from_str(&env, "FeeOnTransferToken")
        }
        pub fn symbol(env: Env) -> String {
            String::from_str(&env, "FOT")
        }
        pub fn burn(env: Env, from: Address, amount: i128) {
            from.require_auth();
            let bal = get_balance(&env, &from);
            set_balance(&env, &from, bal - amount);
        }
        pub fn burn_from(_env: Env, _spender: Address, _from: Address, _amount: i128) {}
        pub fn transfer_from(
            _env: Env,
            _spender: Address,
            _from: Address,
            _to: Address,
            _amount: i128,
        ) {
        }
        pub fn set_authorized(_env: Env, _id: Address, _authorize: bool) {}
        pub fn authorized(_env: Env, _id: Address) -> bool {
            true
        }
    }
}
pub use fee_on_transfer_token::{FeeOnTransferToken, FeeOnTransferTokenClient};

// ─── RevertOnTransferToken ───────────────────────────────────────────────────

pub mod revert_on_transfer_token {
    use super::*;
    /// A token whose `transfer()` always panics with `TransferReverted`.
    ///
    /// **Vulnerability exercised (issue #115):** Any cross-contract call that
    /// tries to pull collateral or pay a premium through this token will fail
    /// at the token boundary. Confirms that options_market / vault propagate
    /// the panic rather than silently continuing.
    #[contract]
    pub struct RevertOnTransferToken;

    #[contractimpl]
    impl RevertOnTransferToken {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            set_balance(&env, &admin, 1_000_000_000_000);
        }

        pub fn mint(env: Env, to: Address, amount: i128) {
            let bal = get_balance(&env, &to);
            set_balance(&env, &to, bal + amount);
        }

        /// Always reverts — no token ever leaves or arrives.
        pub fn transfer(env: Env, from: Address, _to: Address, _amount: i128) {
            from.require_auth();
            panic_with_error!(env, TokenError::TransferReverted);
        }

        pub fn balance(env: Env, id: Address) -> i128 {
            get_balance(&env, &id)
        }

        pub fn approve(
            env: Env,
            from: Address,
            spender: Address,
            amount: i128,
            _expiration_ledger: u32,
        ) {
            from.require_auth();
            env.storage()
                .instance()
                .set(&TokenKey::Allowance(from, spender), &amount);
        }

        pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
            env.storage()
                .instance()
                .get(&TokenKey::Allowance(from, spender))
                .unwrap_or(0i128)
        }

        pub fn decimals(_env: Env) -> u32 {
            7
        }
        pub fn name(env: Env) -> String {
            String::from_str(&env, "RevertOnTransferToken")
        }
        pub fn symbol(env: Env) -> String {
            String::from_str(&env, "REVT")
        }
        pub fn burn(_env: Env, _from: Address, _amount: i128) {}
        pub fn burn_from(_env: Env, _spender: Address, _from: Address, _amount: i128) {}
        pub fn transfer_from(
            _env: Env,
            _spender: Address,
            _from: Address,
            _to: Address,
            _amount: i128,
        ) {
        }
        pub fn set_authorized(_env: Env, _id: Address, _authorize: bool) {}
        pub fn authorized(_env: Env, _id: Address) -> bool {
            true
        }
    }
}
pub use revert_on_transfer_token::{RevertOnTransferToken, RevertOnTransferTokenClient};

// ─── ReentrantToken ──────────────────────────────────────────────────────────

pub mod reentrant_token {
    use super::*;
    /// A token that, on `transfer`, attempts a re-entrant call back into a
    /// pre-configured `target` contract before completing the transfer.
    ///
    /// **Vulnerability exercised (issue #115):** Soroban enforces single-contract
    /// reentrancy at the host level, so the re-entrant call should either be
    /// rejected or serialised. Tests confirm that reentrancy does NOT allow
    /// double-spending or state corruption in vault / options_market.
    ///
    /// Usage: call `set_reentry_target(env, target, function)` before the test
    /// that should trigger the callback.
    #[contract]
    pub struct ReentrantToken;

    #[contracttype]
    enum ReentrantKey {
        Target,
        ReentryFunction,
        ReentryArgs,
        ReentrySucceeded,
        Balances(Address),
        Allowances(Address, Address),
    }

    #[contractimpl]
    impl ReentrantToken {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            env.storage().instance().set(
                &ReentrantKey::Balances(admin.clone()),
                &1_000_000_000_000i128,
            );
        }

        pub fn mint(env: Env, to: Address, amount: i128) {
            let bal: i128 = env
                .storage()
                .instance()
                .get(&ReentrantKey::Balances(to.clone()))
                .unwrap_or(0i128);
            env.storage()
                .instance()
                .set(&ReentrantKey::Balances(to), &(bal + amount));
        }

        /// Configure which contract + function to call back into during a
        /// transfer. Pass the options_market or vault address here.
        pub fn set_reentry_target(env: Env, target: Address, function: soroban_sdk::Symbol) {
            env.storage().instance().set(&ReentrantKey::Target, &target);
            env.storage()
                .instance()
                .set(&ReentrantKey::ReentryFunction, &function);
        }

        /// Arguments for the re-entrant call (default: none).
        pub fn set_reentry_args(env: Env, args: soroban_sdk::Vec<Val>) {
            env.storage()
                .instance()
                .set(&ReentrantKey::ReentryArgs, &args);
        }

        /// `Some(true)` if the last re-entrant call went through,
        /// `Some(false)` if the host rejected it, `None` if none was attempted.
        pub fn reentry_succeeded(env: Env) -> Option<bool> {
            env.storage()
                .instance()
                .get(&ReentrantKey::ReentrySucceeded)
        }

        /// Executes the transfer and fires a re-entrant call if a target is set.
        /// The re-entrant call is attempted with an empty arg list; Soroban's
        /// host will reject it (ContractError or budget exceeded) but the
        /// transfer itself still completes — the point is to confirm the
        /// OUTER contract's state is consistent even if the re-entrant call
        /// could theoretically land.
        pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
            from.require_auth();
            let from_bal: i128 = env
                .storage()
                .instance()
                .get(&ReentrantKey::Balances(from.clone()))
                .unwrap_or(0i128);
            let to_bal: i128 = env
                .storage()
                .instance()
                .get(&ReentrantKey::Balances(to.clone()))
                .unwrap_or(0i128);
            env.storage()
                .instance()
                .set(&ReentrantKey::Balances(from.clone()), &(from_bal - amount));
            env.storage()
                .instance()
                .set(&ReentrantKey::Balances(to.clone()), &(to_bal + amount));

            // Attempt re-entry if a target has been registered.
            if let Some(target) = env
                .storage()
                .instance()
                .get::<_, Address>(&ReentrantKey::Target)
            {
                if let Some(func) = env
                    .storage()
                    .instance()
                    .get::<_, soroban_sdk::Symbol>(&ReentrantKey::ReentryFunction)
                {
                    // Soroban forbids contract-level reentrancy; this will
                    // panic inside invoke_contract. We catch the panic via a
                    // best-effort try_invoke_contract pattern — in testutils
                    // the panic propagates, so tests should wrap this in
                    // should_panic or use try_* client methods.
                    let args: soroban_sdk::Vec<Val> = env
                        .storage()
                        .instance()
                        .get(&ReentrantKey::ReentryArgs)
                        .unwrap_or_else(|| soroban_sdk::vec![&env]);
                    let res =
                        env.try_invoke_contract::<Val, soroban_sdk::Error>(&target, &func, args);
                    env.storage()
                        .instance()
                        .set(&ReentrantKey::ReentrySucceeded, &res.is_ok());
                }
            }
        }

        pub fn balance(env: Env, id: Address) -> i128 {
            env.storage()
                .instance()
                .get(&ReentrantKey::Balances(id))
                .unwrap_or(0i128)
        }

        pub fn approve(
            env: Env,
            from: Address,
            spender: Address,
            amount: i128,
            _expiration_ledger: u32,
        ) {
            from.require_auth();
            env.storage()
                .instance()
                .set(&ReentrantKey::Allowances(from, spender), &amount);
        }

        pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
            env.storage()
                .instance()
                .get(&ReentrantKey::Allowances(from, spender))
                .unwrap_or(0i128)
        }

        pub fn decimals(_env: Env) -> u32 {
            7
        }
        pub fn name(env: Env) -> String {
            String::from_str(&env, "ReentrantToken")
        }
        pub fn symbol(env: Env) -> String {
            String::from_str(&env, "REENT")
        }
        pub fn burn(_env: Env, _from: Address, _amount: i128) {}
        pub fn burn_from(_env: Env, _spender: Address, _from: Address, _amount: i128) {}
        pub fn transfer_from(
            _env: Env,
            _spender: Address,
            _from: Address,
            _to: Address,
            _amount: i128,
        ) {
        }
        pub fn set_authorized(_env: Env, _id: Address, _authorize: bool) {}
        pub fn authorized(_env: Env, _id: Address) -> bool {
            true
        }
    }
}
pub use reentrant_token::{ReentrantToken, ReentrantTokenClient};

// ─── ZeroBalanceToken ─────────────────────────────────────────────────────────

pub mod zero_balance_token {
    use super::*;
    /// A token whose `balance()` always returns 0, even after receiving tokens.
    /// The internal bookkeeping is honest, but `balance()` lies by always
    /// returning zero — the inverse of `LyingBalanceToken`.
    ///
    /// **Vulnerability exercised (issue #115):** If vault's `safe_inbound_transfer`
    /// measures the balance delta after a transfer and the token reports 0 both
    /// before and after, the delta is 0 — which should fail the `received < amount`
    /// guard. Confirms the guard catches zero-delta deposits.
    #[contract]
    pub struct ZeroBalanceToken;

    #[contractimpl]
    impl ZeroBalanceToken {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            set_balance(&env, &admin, 1_000_000_000_000);
        }

        pub fn mint(env: Env, to: Address, amount: i128) {
            let bal = get_balance(&env, &to);
            set_balance(&env, &to, bal + amount);
        }

        pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
            from.require_auth();
            let from_bal = get_balance(&env, &from);
            let to_bal = get_balance(&env, &to);
            set_balance(&env, &from, from_bal - amount);
            set_balance(&env, &to, to_bal + amount);
        }

        /// Always returns 0, regardless of actual balance.
        pub fn balance(_env: Env, _id: Address) -> i128 {
            0
        }

        pub fn approve(
            env: Env,
            from: Address,
            spender: Address,
            amount: i128,
            _expiration_ledger: u32,
        ) {
            from.require_auth();
            env.storage()
                .instance()
                .set(&TokenKey::Allowance(from, spender), &amount);
        }

        pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
            env.storage()
                .instance()
                .get(&TokenKey::Allowance(from, spender))
                .unwrap_or(0i128)
        }

        pub fn decimals(_env: Env) -> u32 {
            7
        }
        pub fn name(env: Env) -> String {
            String::from_str(&env, "ZeroBalanceToken")
        }
        pub fn symbol(env: Env) -> String {
            String::from_str(&env, "ZERO")
        }
        pub fn burn(_env: Env, _from: Address, _amount: i128) {}
        pub fn burn_from(_env: Env, _spender: Address, _from: Address, _amount: i128) {}
        pub fn transfer_from(
            _env: Env,
            _spender: Address,
            _from: Address,
            _to: Address,
            _amount: i128,
        ) {
        }
        pub fn set_authorized(_env: Env, _id: Address, _authorize: bool) {}
        pub fn authorized(_env: Env, _id: Address) -> bool {
            true
        }
    }
}
pub use zero_balance_token::{ZeroBalanceToken, ZeroBalanceTokenClient};

// ─── OverflowToken ────────────────────────────────────────────────────────────

pub mod overflow_token {
    use super::*;
    /// A token whose `balance()` returns `i128::MAX` and whose `transfer()`
    /// silently succeeds without updating any state. Intended to probe
    /// arithmetic paths that handle extreme balance values.
    ///
    /// **Vulnerability exercised (issue #115):** Any arithmetic on a balance of
    /// `i128::MAX` that isn't saturating/checked will overflow. Confirms that
    /// options_market and vault use checked arithmetic throughout payout math.
    #[contract]
    pub struct OverflowToken;

    #[contractimpl]
    impl OverflowToken {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            set_balance(&env, &admin, i128::MAX);
        }

        pub fn mint(env: Env, to: Address, amount: i128) {
            let _ = env;
            let _ = to;
            let _ = amount;
            // no-op — balance is always MAX
        }

        /// No-op: always reports success without moving tokens.
        pub fn transfer(_env: Env, from: Address, _to: Address, _amount: i128) {
            from.require_auth();
            // intentionally no-op
        }

        /// Always returns `i128::MAX`.
        pub fn balance(_env: Env, _id: Address) -> i128 {
            i128::MAX
        }

        pub fn approve(
            env: Env,
            from: Address,
            spender: Address,
            amount: i128,
            _expiration_ledger: u32,
        ) {
            from.require_auth();
            env.storage()
                .instance()
                .set(&TokenKey::Allowance(from, spender), &amount);
        }

        pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
            env.storage()
                .instance()
                .get(&TokenKey::Allowance(from, spender))
                .unwrap_or(i128::MAX)
        }

        pub fn decimals(_env: Env) -> u32 {
            7
        }
        pub fn name(env: Env) -> String {
            String::from_str(&env, "OverflowToken")
        }
        pub fn symbol(env: Env) -> String {
            String::from_str(&env, "OVF")
        }
        pub fn burn(_env: Env, _from: Address, _amount: i128) {}
        pub fn burn_from(_env: Env, _spender: Address, _from: Address, _amount: i128) {}
        pub fn transfer_from(
            _env: Env,
            _spender: Address,
            _from: Address,
            _to: Address,
            _amount: i128,
        ) {
        }
        pub fn set_authorized(_env: Env, _id: Address, _authorize: bool) {}
        pub fn authorized(_env: Env, _id: Address) -> bool {
            true
        }
    }
}
pub use overflow_token::{OverflowToken, OverflowTokenClient};

// ─── NoMoveToken ─────────────────────────────────────────────────────────────

pub mod no_move_token {
    use super::*;
    /// A token whose `transfer` returns cleanly without moving anything — the
    /// SEP-41 analogue of an ERC-20 that returns `true` (or nothing) without
    /// acting. Balances are honest; only `transfer` is a no-op.
    ///
    /// **Vulnerability exercised (issue #264):** a contract that credits
    /// `amount` after a non-reverting `transfer`, instead of measuring the
    /// balance delta, over-credits the depositor for tokens that never arrived.
    #[contract]
    pub struct NoMoveToken;

    #[contractimpl]
    impl NoMoveToken {
        pub fn mint(env: Env, to: Address, amount: i128) {
            let bal = get_balance(&env, &to);
            set_balance(&env, &to, bal + amount);
        }

        pub fn transfer(_env: Env, from: Address, _to: Address, _amount: i128) {
            from.require_auth();
        }

        pub fn balance(env: Env, id: Address) -> i128 {
            get_balance(&env, &id)
        }
    }
}
pub use no_move_token::{NoMoveToken, NoMoveTokenClient};

// ─── BlacklistToken ──────────────────────────────────────────────────────────

pub mod blacklist_token {
    use super::*;
    /// A token (think regulated stablecoin) that reverts any transfer touching
    /// a blacklisted address, in either direction. Otherwise honest.
    ///
    /// **Vulnerability exercised (issue #264):** the blacklist can flip between
    /// a contract's deposit and its payout. The payout must fail atomically,
    /// leaving the contract's recorded balances exactly as they were.
    #[contract]
    pub struct BlacklistToken;

    #[contracttype]
    enum BlacklistKey {
        Blocked(Address),
    }

    #[contractimpl]
    impl BlacklistToken {
        pub fn mint(env: Env, to: Address, amount: i128) {
            let bal = get_balance(&env, &to);
            set_balance(&env, &to, bal + amount);
        }

        pub fn set_blacklisted(env: Env, who: Address, blocked: bool) {
            env.storage()
                .instance()
                .set(&BlacklistKey::Blocked(who), &blocked);
        }

        pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
            from.require_auth();
            for who in [&from, &to] {
                if env
                    .storage()
                    .instance()
                    .get(&BlacklistKey::Blocked(who.clone()))
                    .unwrap_or(false)
                {
                    panic_with_error!(&env, TokenError::TransferReverted);
                }
            }
            let from_bal = get_balance(&env, &from);
            if from_bal < amount {
                panic_with_error!(&env, TokenError::InsufficientBalance);
            }
            set_balance(&env, &from, from_bal - amount);
            let to_bal = get_balance(&env, &to);
            set_balance(&env, &to, to_bal + amount);
        }

        pub fn balance(env: Env, id: Address) -> i128 {
            get_balance(&env, &id)
        }
    }
}
pub use blacklist_token::{BlacklistToken, BlacklistTokenClient};
