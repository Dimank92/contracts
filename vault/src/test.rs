use super::*;
use soroban_sdk::{testutils::Address as _, Address, Env, Vec};

fn setup() -> (Env, VaultClient<'static>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(Vault, ());
    let client = VaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let token = Address::generate(&env);
    client.initialize(&admin, &token);
    (env, client, admin, token)
}

#[test]
fn test_balance_reflects_per_tag_deposits() {
    let (env, client, _admin, token) = setup();
    let user = Address::generate(&env);
    let tag_a = 1u32;
    let tag_b = 2u32;

    client.deposit(&user, &tag_a, &100i128);
    client.deposit(&user, &tag_b, &250i128);

    assert_eq!(client.balance(&token, &tag_a), 100i128);
    assert_eq!(client.balance(&token, &tag_b), 250i128);
}

#[test]
fn test_balance_unknown_tag_is_zero() {
    let (_env, client, _admin, token) = setup();
    assert_eq!(client.balance(&token, &999u32), 0i128);
}

#[test]
fn test_balance_unaffected_by_other_tag_mutations() {
    let (env, client, _admin, token) = setup();
    let user = Address::generate(&env);
    let tag_a = 1u32;
    let tag_b = 2u32;

    client.deposit(&user, &tag_a, &100i128);
    client.deposit(&user, &tag_b, &250i128);

    client.withdraw(&user, &tag_b, &50i128);

    assert_eq!(client.balance(&token, &tag_a), 100i128);
    assert_eq!(client.balance(&token, &tag_b), 200i128);
}

#[test]
fn test_balance_aggregate_over_tags() {
    let (env, client, _admin, token) = setup();
    let user = Address::generate(&env);
    let tag_a = 1u32;
    let tag_b = 2u32;
    let tag_c = 3u32;

    client.deposit(&user, &tag_a, &100i128);
    client.deposit(&user, &tag_b, &250i128);
    client.deposit(&user, &tag_c, &50i128);

    let mut tags = Vec::new(&env);
    tags.push_back(tag_a);
    tags.push_back(tag_b);
    tags.push_back(tag_c);

    assert_eq!(client.balance_of_tags(&token, &tags), 400i128);
}

#[test]
fn test_balance_aggregate_ignores_unknown_tags() {
    let (env, client, _admin, token) = setup();
    let user = Address::generate(&env);
    let tag_a = 1u32;

    client.deposit(&user, &tag_a, &100i128);

    let mut tags = Vec::new(&env);
    tags.push_back(tag_a);
    tags.push_back(999u32);

    assert_eq!(client.balance_of_tags(&token, &tags), 100i128);
}
