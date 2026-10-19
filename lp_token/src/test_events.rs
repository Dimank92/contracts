#![cfg(test)]

//! Events (#126): `initialized` follows schema v2; the SEP-41 events keep
//! the standard layout. All conform to events.json.

extern crate std;

use crate::{InitializedEvent, LpToken, LpTokenClient};
use event_spec::{events_named, last_event, Recorder};
use soroban_sdk::{testutils::Address as _, Address, Env, IntoVal, String, Symbol, TryFromVal};

#[test]
fn every_event_conforms_to_the_spec() {
    let env = Env::default();
    env.mock_all_auths();
    let pool = Address::generate(&env);
    let token = LpTokenClient::new(&env, &env.register_contract(None, LpToken));
    let mut rec = Recorder::new("lp_token", &token.address);
    let (a, b) = (Address::generate(&env), Address::generate(&env));

    token.initialize(
        &pool,
        &7,
        &String::from_str(&env, "Zenith LP Share"),
        &String::from_str(&env, "zLP"),
    );
    rec.record(&env);
    assert_eq!(
        events_named::<InitializedEvent>(&env, &token.address, "initialized"),
        [InitializedEvent {
            pool: pool.clone(),
            decimals: 7,
            name: String::from_str(&env, "Zenith LP Share"),
            symbol: String::from_str(&env, "zLP"),
        }]
    );

    token.mint(&a, &100);
    rec.record(&env);
    let (topics, data) = last_event(&env, &token.address);
    assert_eq!(
        topics,
        (Symbol::new(&env, "mint"), pool, a.clone()).into_val(&env)
    );
    assert_eq!(i128::try_from_val(&env, &data).unwrap(), 100);

    token.approve(&a, &b, &40, &1_000);
    rec.record(&env);
    let (_, data) = last_event(&env, &token.address);
    assert_eq!(
        <(i128, u32)>::try_from_val(&env, &data).unwrap(),
        (40, 1_000)
    );

    token.transfer_from(&b, &a, &b, &30);
    rec.record(&env);
    let (topics, data) = last_event(&env, &token.address);
    assert_eq!(
        topics,
        (Symbol::new(&env, "transfer"), a.clone(), b.clone()).into_val(&env)
    );
    assert_eq!(i128::try_from_val(&env, &data).unwrap(), 30);

    token.burn(&b, &5);
    rec.record(&env);
    let (_, data) = last_event(&env, &token.address);
    assert_eq!(i128::try_from_val(&env, &data).unwrap(), 5);

    rec.assert_complete();
}
