#![cfg(test)]

//! Event schema v2 (#126): every streams event conforms to events.json
//! and carries the documented data.

extern crate std;

use crate::{
    InitializedEvent, StreamCanceledEvent, StreamCreatedEvent, StreamTransferredEvent, Streams,
    StreamsClient, WithdrawnEvent, CURRENT_SCHEMA_VERSION,
};
use event_spec::{events_named, Recorder};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env,
};

#[test]
fn every_event_conforms_to_the_spec() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let (governance, funder) = (Address::generate(&env), Address::generate(&env));
    let (recipient, next) = (Address::generate(&env), Address::generate(&env));
    token::StellarAssetClient::new(&env, &token).mint(&funder, &10_000);
    let client = StreamsClient::new(&env, &env.register_contract(None, Streams));
    let mut rec = Recorder::new("streams", &client.address);

    client.initialize(&governance);
    rec.record(&env);
    assert_eq!(
        events_named::<InitializedEvent>(&env, &client.address, "initialized"),
        [InitializedEvent {
            governance: governance.clone(),
            schema_version: CURRENT_SCHEMA_VERSION,
        }]
    );

    let id = client.create_stream(&funder, &recipient, &token, &1_000, &1_000, &1_000, &2_000);
    rec.record(&env);
    assert_eq!(
        events_named::<StreamCreatedEvent>(&env, &client.address, "stream_created"),
        [StreamCreatedEvent {
            funder: funder.clone(),
            recipient: recipient.clone(),
            token,
            total: 1_000,
            start: 1_000,
            cliff: 1_000,
            end: 2_000,
        }]
    );

    env.ledger().set_timestamp(1_300);
    client.withdraw(&id, &100);
    rec.record(&env);
    assert_eq!(
        events_named::<WithdrawnEvent>(&env, &client.address, "withdrawn"),
        [WithdrawnEvent {
            recipient: recipient.clone(),
            amount: 100,
            withdrawn: 100,
        }]
    );

    client.transfer(&id, &next);
    rec.record(&env);
    assert_eq!(
        events_named::<StreamTransferredEvent>(&env, &client.address, "stream_transferred"),
        [StreamTransferredEvent {
            from: recipient,
            to: next,
        }]
    );

    env.ledger().set_timestamp(1_500);
    client.cancel(&governance, &id);
    rec.record(&env);
    assert_eq!(
        events_named::<StreamCanceledEvent>(&env, &client.address, "stream_canceled"),
        [StreamCanceledEvent {
            caller: governance,
            to_recipient: 400,
            to_funder: 500,
            streamed: 500,
        }]
    );

    rec.assert_complete();
}
