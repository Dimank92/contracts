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

#[cfg(any(test, feature = "testutils"))]
pub mod testutils;

#[cfg(test)]
mod test;

use soroban_sdk::{contracttype, panic_with_error, vec, Address, Env, IntoVal, Symbol, Val};

/// Mirrors multisig's `ActionClass` (same `u32` encoding) without
/// exporting a spec entry into consumer contracts.
#[contracttype(export = false)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ActionClass {
    Emergency = 0,
    Standard = 1,
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

pub fn get_admin<K: IntoVal<Env, Val>>(env: &Env, admin_key: &K) -> Address {
    env.storage().instance().get(admin_key).unwrap()
}

/// Stores `new_admin` and returns the previous admin, for the caller's
/// own `admin_transferred` event.
pub fn set_admin<K: IntoVal<Env, Val>>(env: &Env, admin_key: &K, new_admin: &Address) -> Address {
    let old = get_admin(env, admin_key);
    env.storage().instance().set(admin_key, new_admin);
    old
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

/// Test-only ledger time-travel harness.
///
/// Thin wrappers over `env.ledger().with_mut(...)` so every time-dependent
/// test (expiry, delays, vesting, unbonding, feed freshness, approval TTLs)
/// moves time through one reviewable mechanism instead of ad-hoc
/// `env.ledger().set_timestamp(...)` calls.
///
/// Design rules:
/// - No global mutable state and no implicit offsets: callers always state
///   whether they are advancing seconds, ledgers, or both.
/// - Timestamps never move backwards: `set_timestamp` and
///   `advance_timestamp` assert monotonic non-decreasing time.
/// - TTL is measured in ledgers while business logic is measured in
///   seconds, so `advance` requires both values explicitly.
///
/// Gated behind `#[cfg(any(test, feature = "testutils"))]` so it never
/// inflates wasm size.
#[cfg(any(test, feature = "testutils"))]
pub mod testutils {
    use soroban_sdk::Env;

    /// Returns the current ledger timestamp (seconds).
    pub fn timestamp(env: &Env) -> u64 {
        env.ledger().timestamp()
    }

    /// Returns the current ledger sequence number.
    pub fn sequence(env: &Env) -> u32 {
        env.ledger().sequence()
    }

    /// Sets the absolute ledger timestamp (seconds).
    ///
    /// Panics if `ts` is earlier than the current timestamp: Soroban
    /// timestamps must not move backwards.
    pub fn set_timestamp(env: &Env, ts: u64) {
        let now = env.ledger().timestamp();
        assert!(
            ts >= now,
            "ledger timestamp must not move backwards: now={}, requested={}",
            now,
            ts
        );
        env.ledger().with_mut(|li| li.timestamp = ts);
    }

    /// Advances the ledger timestamp by `delta` seconds.
    ///
    /// A zero delta is a no-op; the monotonicity assertion in
    /// `set_timestamp` guards the rest.
    pub fn advance_timestamp(env: &Env, delta: u64) {
        let now = env.ledger().timestamp();
        let next = now
            .checked_add(delta)
            .expect("ledger timestamp overflow while advancing");
        set_timestamp(env, next);
    }

    /// Advances the ledger sequence number by `delta` ledgers.
    ///
    /// This is what TTL extension and expiry-in-ledgers logic observes.
    pub fn advance_sequence(env: &Env, delta: u32) {
        env.ledger().with_mut(|li| {
            li.sequence_number = li
                .sequence_number
                .checked_add(delta)
                .expect("ledger sequence overflow while advancing");
        });
    }

    /// Advances both time and sequence, requiring both explicitly.
    ///
    /// `seconds` moves the timestamp; `ledgers` moves the sequence number.
    /// There is deliberately no implicit ratio between the two: TTL is
    /// measured in ledgers while business logic is measured in seconds,
    /// and callers must state both.
    pub fn advance(env: &Env, seconds: u64, ledgers: u32) {
        advance_timestamp(env, seconds);
        advance_sequence(env, ledgers);
    }

    /// Asserts that `now` is strictly before `deadline` (not yet expired).
    pub fn assert_not_expired(now: u64, deadline: u64) {
        assert!(
            now < deadline,
            "expected not expired: now={}, deadline={}",
            now,
            deadline
        );
    }

    /// Asserts that `now` is at or past `deadline` (expired).
    pub fn assert_expired(now: u64, deadline: u64) {
        assert!(
            now >= deadline,
            "expected expired: now={}, deadline={}",
            now,
            deadline
        );
    }

    /// Asserts that a report observed at `observed_at` is still fresh at
    /// `now`, i.e. `now - observed_at <= max_age`.
    pub fn assert_fresh(now: u64, observed_at: u64, max_age: u64) {
        assert!(
            now >= observed_at,
            "observation is in the future: now={}, observed_at={}",
            now,
            observed_at
        );
        let age = now - observed_at;
        assert!(
            age <= max_age,
            "expected fresh: age={}, max_age={}",
            age,
            max_age
        );
    }

    /// Asserts that a report observed at `observed_at` is stale at `now`,
    /// i.e. `now - observed_at > max_age`.
    pub fn assert_stale(now: u64, observed_at: u64, max_age: u64) {
        assert!(
            now >= observed_at,
            "observation is in the future: now={}, observed_at={}",
            now,
            observed_at
        );
        let age = now - observed_at;
        assert!(
            age > max_age,
            "expected stale: age={}, max_age={}",
            age,
            max_age
        );
    }

    /// Asserts that a TTL measured in ledgers has not yet elapsed:
    /// `current_sequence < expires_at_sequence`.
    pub fn assert_ttl_live(current_sequence: u32, expires_at_sequence: u32) {
        assert!(
            current_sequence < expires_at_sequence,
            "expected live TTL: sequence={}, expires_at={}",
            current_sequence,
            expires_at_sequence
        );
    }

    /// Asserts that a TTL measured in ledgers has elapsed:
    /// `current_sequence >= expires_at_sequence`.
    pub fn assert_ttl_expired(current_sequence: u32, expires_at_sequence: u32) {
        assert!(
            current_sequence >= expires_at_sequence,
            "expected expired TTL: sequence={}, expires_at={}",
            current_sequence,
            expires_at_sequence
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use soroban_sdk::Env;

        #[test]
        fn set_timestamp_moves_forward() {
            let env = Env::default();
            let start = timestamp(&env);
            set_timestamp(&env, start + 100);
            assert_eq!(timestamp(&env), start + 100);
        }

        #[test]
        fn set_timestamp_allows_same_value() {
            let env = Env::default();
            let start = timestamp(&env);
            set_timestamp(&env, start);
            assert_eq!(timestamp(&env), start);
        }

        #[test]
        #[should_panic(expected = "must not move backwards")]
        fn set_timestamp_rejects_backwards_time() {
            let env = Env::default();
            let start = timestamp(&env);
            set_timestamp(&env, start + 10);
            set_timestamp(&env, start);
        }

        #[test]
        fn advance_timestamp_adds_delta() {
            let env = Env::default();
            let start = timestamp(&env);
            advance_timestamp(&env, 0);
            assert_eq!(timestamp(&env), start);
            advance_timestamp(&env, 42);
            assert_eq!(timestamp(&env), start + 42);
            advance_timestamp(&env, 8);
            assert_eq!(timestamp(&env), start + 50);
        }

        #[test]
        fn advance_sequence_adds_delta() {
            let env = Env::default();
            let start = sequence(&env);
            advance_sequence(&env, 0);
            assert_eq!(sequence(&env), start);
            advance_sequence(&env, 7);
            assert_eq!(sequence(&env), start + 7);
        }

        #[test]
        fn advance_moves_both_explicitly() {
            let env = Env::default();
            let start_ts = timestamp(&env);
            let start_seq = sequence(&env);
            advance(&env, 60, 5);
            assert_eq!(timestamp(&env), start_ts + 60);
            assert_eq!(sequence(&env), start_seq + 5);
        }

        #[test]
        fn boundary_assertions_accept_valid_cases() {
            assert_not_expired(99, 100);
            assert_expired(100, 100);
            assert_expired(101, 100);
            assert_fresh(150, 100, 50);
            assert_stale(151, 100, 50);
            assert_ttl_live(9, 10);
            assert_ttl_expired(10, 10);
        }

        #[test]
        #[should_panic(expected = "expected expired")]
        fn assert_expired_panics_before_deadline() {
            assert_expired(99, 100);
        }

        #[test]
        #[should_panic(expected = "expected stale")]
        fn assert_stale_panics_when_fresh() {
            assert_stale(100, 100, 50);
        }

        #[test]
        #[should_panic(expected = "expected live TTL")]
        fn assert_ttl_live_panics_when_elapsed() {
            assert_ttl_live(10, 10);
        }
    }
}
