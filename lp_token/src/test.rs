#![cfg(test)]

//! SEP-41 conformance suite, modeled on soroban-examples' token tests.

extern crate std;

use crate::{Error, LpToken, LpTokenClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation, Ledger as _},
    Address, Env, IntoVal, String, Symbol, TryFromVal,
};

fn setup<'a>() -> (Env, LpTokenClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let pool = Address::generate(&env);
    let client = LpTokenClient::new(&env, &env.register_contract(None, LpToken));
    client.initialize(
        &pool,
        &7,
        &String::from_str(&env, "Zenith LP Share"),
        &String::from_str(&env, "zLP"),
    );
    (env, client, pool)
}

/// SEP-41 functions keep their non-`Result` signatures, so a failed
/// `try_*` call reports the host-level error for the contract code.
fn sep41_err(e: Error) -> soroban_sdk::Error {
    e.into()
}

fn last_event_topic(env: &Env, token: &Address) -> Symbol {
    let (topics, _) = event_spec::last_event(env, token);
    Symbol::try_from_val(env, &topics.get(0).unwrap()).unwrap()
}

#[test]
fn metadata() {
    let (env, token, pool) = setup();
    assert_eq!(token.decimals(), 7);
    assert_eq!(token.name(), String::from_str(&env, "Zenith LP Share"));
    assert_eq!(token.symbol(), String::from_str(&env, "zLP"));
    assert_eq!(token.pool(), pool);
}

#[test]
fn mint_requires_the_pool_and_emits_mint() {
    let (env, token, pool) = setup();
    let user = Address::generate(&env);
    token.mint(&user, &1000);
    assert_eq!(
        env.auths(),
        std::vec![(
            pool.clone(),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    token.address.clone(),
                    symbol_short!("mint"),
                    (&user, 1000_i128).into_val(&env),
                )),
                sub_invocations: std::vec![],
            }
        )]
    );
    assert_eq!(token.balance(&user), 1000);
    assert_eq!(
        last_event_topic(&env, &token.address),
        symbol_short!("mint")
    );
}

#[test]
fn mint_without_pool_auth_fails() {
    let env = Env::default();
    let pool = Address::generate(&env);
    let token = LpTokenClient::new(&env, &env.register_contract(None, LpToken));
    env.mock_all_auths();
    token.initialize(
        &pool,
        &7,
        &String::from_str(&env, "n"),
        &String::from_str(&env, "s"),
    );
    env.set_auths(&[]);
    // No pool signature: the host rejects the auth check.
    assert!(matches!(
        token.try_mint(&Address::generate(&env), &1),
        Err(Err(_))
    ));
}

#[test]
fn uninitialized_token_reports_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let token = LpTokenClient::new(&env, &env.register_contract(None, LpToken));
    assert_eq!(token.try_pool(), Err(Ok(Error::NotInitialized)));
    assert_eq!(
        token.try_mint(&Address::generate(&env), &1),
        Err(Ok(Error::NotInitialized))
    );
    assert_eq!(
        token.try_decimals(),
        Err(Ok(sep41_err(Error::NotInitialized)))
    );
}

#[test]
fn transfer_approve_transfer_from_burn() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let c = Address::generate(&env);
    token.mint(&a, &1000);

    token.approve(&a, &c, &500, &200);
    assert_eq!(
        last_event_topic(&env, &token.address),
        symbol_short!("approve")
    );
    assert_eq!(
        env.auths()[0].1.function,
        AuthorizedFunction::Contract((
            token.address.clone(),
            symbol_short!("approve"),
            (&a, &c, 500_i128, 200_u32).into_val(&env),
        ))
    );
    assert_eq!(token.allowance(&a, &c), 500);

    token.transfer(&a, &b, &600);
    assert_eq!(
        last_event_topic(&env, &token.address),
        symbol_short!("transfer")
    );
    assert_eq!(env.auths()[0].0, a);
    assert_eq!(token.balance(&a), 400);
    assert_eq!(token.balance(&b), 600);

    token.transfer_from(&c, &a, &b, &400);
    assert_eq!(env.auths()[0].0, c);
    assert_eq!(token.balance(&a), 0);
    assert_eq!(token.balance(&b), 1000);
    assert_eq!(token.allowance(&a, &c), 100);

    token.burn(&b, &300);
    assert_eq!(
        last_event_topic(&env, &token.address),
        symbol_short!("burn")
    );
    assert_eq!(token.balance(&b), 700);

    token.approve(&b, &c, &200, &200);
    token.burn_from(&c, &b, &150);
    assert_eq!(env.auths()[0].0, c);
    assert_eq!(token.balance(&b), 550);
    assert_eq!(token.allowance(&b, &c), 50);
}

#[test]
fn allowance_expires_at_its_expiration_ledger() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let c = Address::generate(&env);
    token.mint(&a, &100);
    let now = env.ledger().sequence();
    token.approve(&a, &c, &50, &(now + 10));
    env.ledger().with_mut(|l| l.sequence_number = now + 10);
    assert_eq!(token.allowance(&a, &c), 50);
    env.ledger().with_mut(|l| l.sequence_number = now + 11);
    assert_eq!(token.allowance(&a, &c), 0);
}

#[test]
fn a_zero_allowance_may_carry_a_past_expiration() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let c = Address::generate(&env);
    env.ledger().with_mut(|l| l.sequence_number = 100);
    token.approve(&a, &c, &0, &1);
    assert_eq!(token.allowance(&a, &c), 0);
}

#[test]
fn a_nonzero_allowance_cannot_be_already_expired() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    env.ledger().with_mut(|l| l.sequence_number = 100);
    assert_eq!(
        token.try_approve(&a, &Address::generate(&env), &1, &99),
        Err(Ok(sep41_err(Error::InvalidExpiration)))
    );
}

#[test]
fn transfer_from_an_expired_allowance_fails() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let c = Address::generate(&env);
    token.mint(&a, &100);
    let now = env.ledger().sequence();
    token.approve(&a, &c, &50, &(now + 1));
    env.ledger().with_mut(|l| l.sequence_number = now + 2);
    assert_eq!(
        token.try_transfer_from(&c, &a, &c, &10),
        Err(Ok(sep41_err(Error::InsufficientAllowance)))
    );
}

#[test]
fn transfer_over_balance_fails() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    token.mint(&a, &100);
    assert_eq!(
        token.try_transfer(&a, &Address::generate(&env), &101),
        Err(Ok(sep41_err(Error::InsufficientBalance)))
    );
}

#[test]
fn transfer_from_over_allowance_fails() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let c = Address::generate(&env);
    token.mint(&a, &100);
    token.approve(&a, &c, &10, &1000);
    assert_eq!(
        token.try_transfer_from(&c, &a, &c, &11),
        Err(Ok(sep41_err(Error::InsufficientAllowance)))
    );
}

#[test]
fn burn_from_over_allowance_fails() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let c = Address::generate(&env);
    token.mint(&a, &100);
    assert_eq!(
        token.try_burn_from(&c, &a, &1),
        Err(Ok(sep41_err(Error::InsufficientAllowance)))
    );
}

#[test]
fn negative_amounts_are_rejected() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    assert_eq!(
        token.try_transfer(&a, &Address::generate(&env), &-1),
        Err(Ok(sep41_err(Error::NegativeAmount)))
    );
}

#[test]
fn initialize_twice_fails() {
    let (env, token, pool) = setup();
    assert_eq!(
        token.try_initialize(
            &pool,
            &7,
            &String::from_str(&env, "n"),
            &String::from_str(&env, "s"),
        ),
        Err(Ok(Error::AlreadyInitialized))
    );
}

#[test]
fn decimals_must_fit_in_a_u8() {
    let env = Env::default();
    env.mock_all_auths();
    let token = LpTokenClient::new(&env, &env.register_contract(None, LpToken));
    assert_eq!(
        token.try_initialize(
            &Address::generate(&env),
            &256,
            &String::from_str(&env, "n"),
            &String::from_str(&env, "s"),
        ),
        Err(Ok(Error::InvalidDecimals))
    );
}

#[test]
fn works_through_the_generic_token_client() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    token.mint(&a, &10);
    let generic = soroban_sdk::token::TokenClient::new(&env, &token.address);
    generic.transfer(&a, &b, &4);
    assert_eq!(generic.balance(&b), 4);
    assert_eq!(generic.decimals(), 7);
}
