//! Invocation-local interleaving protection and checks-effects-interactions.
//!
//! Use ONE key for all entrypoints sharing liabilities, not a different key per
//! method: deposit -> withdraw is just as dangerous as deposit -> deposit.
//! Acquire before authorization (including multisig and custom-account auth),
//! token balance reads, oracle reads, or any other untrusted interaction.
//!
//! Normal closure returns, including `?` and early `Err`, remove the flag.
//! A host abort/panic does not execute cleanup: Soroban rolls back the failed
//! invocation's storage writes, including acquisition. Do not catch a panic
//! inside the closure. This is not a persistent lock and requires no TTL work.
//!
//! Returning `Err` from this Rust helper does NOT roll back earlier writes or
//! transfers. Entrypoints must propagate failures as contract errors (or panic)
//! rather than swallow them and commit partially completed operations.

use soroban_sdk::{Env, IntoVal, Symbol, Val};

// The tuple namespace below is reserved for this helper. Callers must not
// write application data under (Symbol("zenith_guard"), key).

/// Execute `f` with an instance-storage lock. A single storage read checks
/// acquisition; re-entry returns `violation` without executing `f` or clearing
/// the outer invocation's lock. Use a distinct contract error for `violation`.
pub fn with_guard<K, T, E>(
    env: &Env,
    key: &K,
    violation: E,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E>
where
    K: IntoVal<Env, Val>,
{
    let key = (Symbol::new(env, "zenith_guard"), key.into_val(env));
    if env
        .storage()
        .instance()
        .get::<_, bool>(&key)
        .unwrap_or(false)
    {
        return Err(violation);
    }
    env.storage().instance().set(&key, &true);
    let result = f();
    env.storage().instance().remove(&key);
    result
}

/// Run checks, then effects, then interactions. Call INSIDE `with_guard`.
/// Checks should be local validation; if authorization/oracle discovery needs
/// a cross-call, hold the guard and finish that discovery before effects.
/// Effects return the data needed by interactions so callers need not borrow
/// partially updated state. Interaction failure must abort the entrypoint to
/// roll effects back. This helper orders stages; it cannot inspect closures
/// for hidden external calls or make a multi-contract operation atomic itself.
pub fn checks_effects_interactions<C, S, T, E>(
    checks: impl FnOnce() -> Result<C, E>,
    effects: impl FnOnce(C) -> Result<S, E>,
    interactions: impl FnOnce(S) -> Result<T, E>,
) -> Result<T, E> {
    let checked = checks()?;
    let state = effects(checked)?;
    interactions(state)
}

#[cfg(test)]
mod test {
    extern crate std;
    use super::*;
    use soroban_sdk::{contract, contractimpl, symbol_short};
    use std::cell::RefCell;

    #[contract]
    struct Harness;

    #[contractimpl]
    impl Harness {
        pub fn abort(env: Env) {
            let _: Result<(), u32> = with_guard(&env, &symbol_short!("escrow"), 99, || {
                panic!("simulated failed token invocation")
            });
        }

        pub fn available(env: Env) -> bool {
            with_guard(&env, &symbol_short!("escrow"), 99u32, || Ok(())).is_ok()
        }
    }

    #[test]
    fn releases_on_success_error_and_question_mark() {
        let env = Env::default();
        let id = env.register_contract(None, Harness);
        env.as_contract(&id, || {
            let key = symbol_short!("escrow");
            assert_eq!(with_guard(&env, &key, 99, || Ok(7)), Ok(7));
            let result: Result<(), u32> = with_guard(&env, &key, 99, || {
                Err(42u32)?;
                Ok(())
            });
            assert_eq!(result, Err(42));
            assert_eq!(with_guard(&env, &key, 99, || Ok(8)), Ok(8));
        });
    }

    #[test]
    fn reentry_never_executes_or_unlocks_outer_guard() {
        let env = Env::default();
        let id = env.register_contract(None, Harness);
        env.as_contract(&id, || {
            let key = symbol_short!("escrow");
            let result = with_guard(&env, &key, 99, || {
                for _ in 0..2 {
                    let nested: Result<(), u32> =
                        with_guard(&env, &key, 99, || panic!("reentry executed"));
                    assert_eq!(nested, Err(99));
                }
                // Independent invocation classes may be used for disjoint state.
                with_guard(&env, &symbol_short!("other"), 99, || Ok(()))
            });
            assert_eq!(result, Ok(()));
            assert_eq!(with_guard(&env, &key, 99, || Ok(())), Ok(()));
        });
    }

    #[test]
    fn failed_invocation_rolls_back_acquisition() {
        let env = Env::default();
        let id = env.register_contract(None, Harness);
        let client = HarnessClient::new(&env, &id);
        assert!(client.try_abort().is_err());
        assert!(client.available());
    }

    #[test]
    fn cei_orders_stages_and_short_circuits() {
        let order = RefCell::new(std::vec::Vec::new());
        let result: Result<u32, u32> = checks_effects_interactions(
            || {
                order.borrow_mut().push(1);
                Ok(10)
            },
            |n| {
                order.borrow_mut().push(2);
                Ok(n + 1)
            },
            |n| {
                order.borrow_mut().push(3);
                Ok(n)
            },
        );
        assert_eq!(result, Ok(11));
        assert_eq!(*order.borrow(), std::vec![1, 2, 3]);
        let result: Result<(), u32> = checks_effects_interactions(
            || Err(42),
            |_: ()| -> Result<(), u32> { panic!("effects after failed check") },
            |_| -> Result<(), u32> { panic!("interaction after failed check") },
        );
        assert_eq!(result, Err(42));
        let result: Result<(), u32> = checks_effects_interactions(
            || Ok(()),
            |_| -> Result<(), u32> { Err(43) },
            |_| -> Result<(), u32> { panic!("interaction after failed effect") },
        );
        assert_eq!(result, Err(43));
    }
}
