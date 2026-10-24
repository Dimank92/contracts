#![cfg(test)]

//! Event schema v2 (#126): every params event conforms to events.json and
//! carries the documented data.

extern crate std;

use crate::{
    BoundsCancelledEvent, BoundsProposedEvent, BoundsUpdatedEvent, InitializedEvent,
    ParamDefinedEvent, ParamUpdatedEvent, Params, ParamsClient, BOUNDS_CHANGE_DELAY,
    CURRENT_SCHEMA_VERSION,
};
use event_spec::{events_named, Recorder};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, Env,
};

#[test]
fn every_event_conforms_to_the_spec() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let timelock = Address::generate(&env);
    let client = ParamsClient::new(&env, &env.register_contract(None, Params));
    let mut rec = Recorder::new("params", &client.address);
    let fee = symbol_short!("fee_bps");

    client.initialize(&timelock);
    rec.record(&env);
    assert_eq!(
        events_named::<InitializedEvent>(&env, &client.address, "initialized"),
        [InitializedEvent {
            timelock: timelock.clone(),
            schema_version: CURRENT_SCHEMA_VERSION,
        }]
    );

    client.define_param(&fee, &50, &0, &100);
    rec.record(&env);
    assert_eq!(
        events_named::<ParamDefinedEvent>(&env, &client.address, "param_defined"),
        [ParamDefinedEvent {
            value: 50,
            min: 0,
            max: 100,
            updated_at: 1_000,
            version: 1,
        }]
    );

    client.set_param(&fee, &60);
    rec.record(&env);
    assert_eq!(
        events_named::<ParamUpdatedEvent>(&env, &client.address, "param_updated"),
        [ParamUpdatedEvent {
            old_value: 50,
            new_value: 60,
            updated_at: 1_000,
            version: 2,
        }]
    );

    client.propose_bounds(&fee, &0, &500);
    rec.record(&env);
    let eta = 1_000 + BOUNDS_CHANGE_DELAY;
    assert_eq!(
        events_named::<BoundsProposedEvent>(&env, &client.address, "bounds_proposed"),
        [BoundsProposedEvent {
            min: 0,
            max: 500,
            eta,
        }]
    );

    client.cancel_bounds(&fee);
    rec.record(&env);
    assert_eq!(
        events_named::<BoundsCancelledEvent>(&env, &client.address, "bounds_cancelled"),
        [BoundsCancelledEvent {
            min: 0,
            max: 500,
            eta,
        }]
    );

    client.propose_bounds(&fee, &10, &200);
    env.ledger().set_timestamp(eta);
    client.execute_bounds(&fee);
    rec.record(&env);
    assert_eq!(
        events_named::<BoundsUpdatedEvent>(&env, &client.address, "bounds_updated"),
        [BoundsUpdatedEvent {
            min: 10,
            max: 200,
            updated_at: eta,
            version: 3,
        }]
    );

    rec.assert_complete();
}
