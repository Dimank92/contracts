#![cfg(test)]

//! SEP-41 conformance suite, modeled on soroban-examples' token tests.

extern crate std;

use crate::{LpToken, LpTokenClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation, Events as _, Ledger as _},
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

fn last_event_topic(env: &Env) -> Symbol {
    let (_, topics, _) = env.events().all().last().unwrap();
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
    assert_eq!(last_event_topic(&env), symbol_short!("mint"));
}

#[test]
#[should_panic]
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
    token.mint(&Address::generate(&env), &1);
}

#[test]
fn transfer_approve_transfer_from_burn() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let c = Address::generate(&env);
    token.mint(&a, &1000);

    token.approve(&a, &c, &500, &200);
    assert_eq!(last_event_topic(&env), symbol_short!("approve"));
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
    assert_eq!(last_event_topic(&env), symbol_short!("transfer"));
    assert_eq!(env.auths()[0].0, a);
    assert_eq!(token.balance(&a), 400);
    assert_eq!(token.balance(&b), 600);

    token.transfer_from(&c, &a, &b, &400);
    assert_eq!(env.auths()[0].0, c);
    assert_eq!(token.balance(&a), 0);
    assert_eq!(token.balance(&b), 1000);
    assert_eq!(token.allowance(&a, &c), 100);

    token.burn(&b, &300);
    assert_eq!(last_event_topic(&env), symbol_short!("burn"));
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
#[should_panic(expected = "Error(Contract, #6)")] // InvalidExpiration
fn a_nonzero_allowance_cannot_be_already_expired() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    env.ledger().with_mut(|l| l.sequence_number = 100);
    token.approve(&a, &Address::generate(&env), &1, &99);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // InsufficientAllowance
fn transfer_from_an_expired_allowance_fails() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let c = Address::generate(&env);
    token.mint(&a, &100);
    let now = env.ledger().sequence();
    token.approve(&a, &c, &50, &(now + 1));
    env.ledger().with_mut(|l| l.sequence_number = now + 2);
    token.transfer_from(&c, &a, &c, &10);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // InsufficientBalance
fn transfer_over_balance_fails() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    token.mint(&a, &100);
    token.transfer(&a, &Address::generate(&env), &101);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // InsufficientAllowance
fn transfer_from_over_allowance_fails() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let c = Address::generate(&env);
    token.mint(&a, &100);
    token.approve(&a, &c, &10, &1000);
    token.transfer_from(&c, &a, &c, &11);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // InsufficientAllowance
fn burn_from_over_allowance_fails() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let c = Address::generate(&env);
    token.mint(&a, &100);
    token.burn_from(&c, &a, &1);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // NegativeAmount
fn negative_amounts_are_rejected() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    token.transfer(&a, &Address::generate(&env), &-1);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn initialize_twice_fails() {
    let (env, token, pool) = setup();
    token.initialize(
        &pool,
        &7,
        &String::from_str(&env, "n"),
        &String::from_str(&env, "s"),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn re_initialization_cannot_re_pin_the_pool() {
    let (env, token, pool) = setup();
    let attacker = Address::generate(&env);
    // Attempt to re-pin to a different pool address; must fail and leave the pin intact.
    let _ = token.try_initialize(
        &attacker,
        &7,
        &String::from_str(&env, "n"),
        &String::from_str(&env, "s"),
    );
    assert_eq!(token.pool(), pool);
    token.initialize(
        &attacker,
        &7,
        &String::from_str(&env, "n"),
        &String::from_str(&env, "s"),
    );
}

#[test]
fn non_pool_caller_cannot_mint() {
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
    let user = Address::generate(&env);
    // Only the pinned pool may mint; a user's authorization is not enough.
    env.set_auths(&[]);
    let res = token.try_mint(&user, &1);
    assert!(res.is_err());
    assert_eq!(token.balance(&user), 0);
}

#[test]
fn non_pool_caller_cannot_burn() {
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
    let user = Address::generate(&env);
    token.mint(&user, &100);
    env.set_auths(&[]);
    let res = token.try_burn(&user, &1);
    assert!(res.is_err());
    assert_eq!(token.balance(&user), 100);
}

#[test]
fn self_transfer_is_a_noop_but_succeeds() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    token.mint(&a, &100);
    token.transfer(&a, &a, &40);
    assert_eq!(token.balance(&a), 100);
    assert_eq!(last_event_topic(&env), symbol_short!("transfer"));
}

#[test]
fn transfer_to_the_contract_address_succeeds() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    token.mint(&a, &100);
    token.transfer(&a, &token.address, &25);
    assert_eq!(token.balance(&a), 75);
    assert_eq!(token.balance(&token.address), 25);
}

#[test]
fn allowance_exactly_equal_to_the_amount_is_consumed_to_zero() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let c = Address::generate(&env);
    token.mint(&a, &100);
    token.approve(&a, &c, &50, &1000);
    token.transfer_from(&c, &a, &c, &50);
    assert_eq!(token.balance(&a), 50);
    assert_eq!(token.balance(&c), 50);
    assert_eq!(token.allowance(&a, &c), 0);
}

#[test]
fn approving_the_zero_address_is_allowed() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    let zero = Address::from_string(&String::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));
    token.approve(&a, &zero, &10, &1000);
    assert_eq!(token.allowance(&a, &zero), 10);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // InsufficientBalance
fn burning_more_than_the_caller_balance_fails() {
    let (env, token, _) = setup();
    let a = Address::generate(&env);
    token.mint(&a, &100);
    token.burn(&a, &101);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // InvalidDecimals
fn decimals_must_fit_in_a_u8() {
    let env = Env::default();
    env.mock_all_auths();
    let token = LpTokenClient::new(&env, &env.register_contract(None, LpToken));
    token.initialize(
        &Address::generate(&env),
        &256,
        &String::from_str(&env, "n"),
        &String::from_str(&env, "s"),
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
