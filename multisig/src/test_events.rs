#![cfg(test)]

//! Event schema v2 (#126): every multisig event conforms to events.json
//! and carries the documented data.

extern crate std;

use crate::test::{Target, TargetClient};
use crate::{
    ActionExpiredEvent, ActionFinishedEvent, ApprovedEvent, ExecuteBySetEvent, InitializedEvent,
    Multisig, MultisigClient, RegisteredEvent, RevokedEvent, SignerChangeQueuedEvent,
    SignerChangeVetoedEvent, SignerWeight, SignersRotatedEvent, SingleKeyQuorumEvent,
    UpgradeApprovedEvent, UpgradeRevokedEvent, VetoedEvent, VetoerSetEvent, CURRENT_SCHEMA_VERSION,
    UPGRADE_DELAY,
};
use event_spec::{events_named, Recorder};
use soroban_sdk::{
    map,
    testutils::{Address as _, Ledger as _},
    vec, Address, BytesN, Env, IntoVal, Symbol, Vec,
};

const TTL: u64 = 1_000;
const ROTATION_DELAY: u64 = 100;

fn id(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

fn zero(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[0; 32])
}

#[test]
fn every_event_conforms_to_the_spec() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(10);
    let (a, b, c) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    let client = MultisigClient::new(&env, &env.register_contract(None, Multisig));
    let mut rec = Recorder::new("multisig", &client.address);
    let at = &client.address;

    // `a` alone meets the threshold of 3: a single_key_quorum warning.
    let signers = vec![
        &env,
        (a.clone(), 3u32),
        (b.clone(), 1u32),
        (c.clone(), 1u32),
    ];
    client.initialize_weighted(&signers, &3, &TTL, &4, &ROTATION_DELAY);
    rec.record(&env);
    assert_eq!(
        events_named::<InitializedEvent>(&env, at, "initialized"),
        [InitializedEvent {
            signers: map![&env, (a.clone(), 3), (b.clone(), 1), (c.clone(), 1)],
            threshold: 3,
            approval_ttl: TTL,
            rotation_threshold: 4,
            rotation_delay: ROTATION_DELAY,
            schema_version: CURRENT_SCHEMA_VERSION,
        }]
    );
    assert_eq!(
        events_named::<SingleKeyQuorumEvent>(&env, at, "single_key_quorum"),
        [SingleKeyQuorumEvent {
            weight: 3,
            threshold: 3,
        }]
    );

    // Registry, approvals, revoke, reset.
    let desc = id(&env, 9);
    client.register_action(&b, &id(&env, 1), &desc);
    client.approve(&b, &id(&env, 1));
    client.approve(&c, &id(&env, 1));
    client.revoke(&c, &id(&env, 1));
    client.approve(&a, &id(&env, 1));
    client.reset(&id(&env, 1));
    rec.record(&env);
    assert_eq!(
        events_named::<RegisteredEvent>(&env, at, "registered")[0],
        RegisteredEvent {
            proposer: b.clone(),
            description_hash: desc.clone(),
            created_at: 10,
        }
    );
    let approved = events_named::<ApprovedEvent>(&env, at, "approved");
    assert_eq!(
        approved[1],
        ApprovedEvent {
            description_hash: desc.clone(),
            approved_at: 10,
            approval_count: 2,
            approval_weight: 2,
            epoch: 0,
        }
    );
    assert_eq!(
        events_named::<RevokedEvent>(&env, at, "revoked"),
        [RevokedEvent {
            description_hash: desc.clone(),
            approval_count: 1,
            approval_weight: 1,
        }]
    );
    assert_eq!(
        events_named::<ActionFinishedEvent>(&env, at, "reset"),
        [ActionFinishedEvent {
            description_hash: desc
        }]
    );

    // Executor: propose, approve, execute.
    let target = TargetClient::new(&env, &env.register_contract(None, Target));
    target.init(&client.address);
    let args: Vec<soroban_sdk::Val> = vec![&env, 7u32.into_val(&env)];
    let proposal = client.propose(&a, &target.address, &Symbol::new(&env, "set"), &args);
    client.approve(&a, &proposal);
    client.execute(&proposal);
    rec.record(&env);
    assert_eq!(target.get(), 7);
    assert_eq!(
        events_named::<ActionFinishedEvent>(&env, at, "executed"),
        [ActionFinishedEvent {
            description_hash: proposal.clone()
        }]
    );

    // Vetoer and veto.
    let vetoer = Address::generate(&env);
    client.set_vetoer(&vetoer);
    client.approve(&b, &id(&env, 2));
    client.veto(&vetoer, &id(&env, 2));
    rec.record(&env);
    assert_eq!(
        events_named::<VetoerSetEvent>(&env, at, "vetoer_set"),
        [VetoerSetEvent {
            vetoer: vetoer.clone()
        }]
    );
    assert_eq!(
        events_named::<VetoedEvent>(&env, at, "vetoed"),
        [VetoedEvent {
            vetoer,
            description_hash: zero(&env),
        }]
    );

    // Deadline: execute_by, then action_expired.
    client.approve(&b, &id(&env, 3));
    client.set_execute_by(&b, &id(&env, 3), &20);
    env.ledger().set_timestamp(21);
    client.expire(&id(&env, 3));
    rec.record(&env);
    assert_eq!(
        events_named::<ExecuteBySetEvent>(&env, at, "execute_by_set"),
        [ExecuteBySetEvent { execute_by: 20 }]
    );
    assert_eq!(
        events_named::<ActionExpiredEvent>(&env, at, "action_expired"),
        [ActionExpiredEvent {
            description_hash: zero(&env),
            execute_by: 20,
        }]
    );

    // Stale approvals: expired.
    client.approve(&b, &id(&env, 4));
    env.ledger().set_timestamp(21 + TTL + 1);
    client.expire(&id(&env, 4));
    rec.record(&env);
    assert_eq!(
        events_named::<ActionFinishedEvent>(&env, at, "expired"),
        [ActionFinishedEvent {
            description_hash: zero(&env)
        }]
    );

    // Rotation: propose, queue, execute.
    let d = Address::generate(&env);
    let add = vec![
        &env,
        SignerWeight {
            address: d.clone(),
            weight: 1,
        },
    ];
    let change = client.propose_signer_change(&a, &add, &Vec::new(&env), &3, &4);
    client.approve(&a, &change);
    client.approve(&b, &change);
    let ready_at = client.queue_signer_change(&change);
    env.ledger().set_timestamp(ready_at);
    client.execute_signer_change(&change);
    rec.record(&env);
    assert_eq!(
        events_named::<SignerChangeQueuedEvent>(&env, at, "signer_change_queued"),
        [SignerChangeQueuedEvent { ready_at }]
    );
    assert_eq!(
        events_named::<SignersRotatedEvent>(&env, at, "signers_rotated"),
        [SignersRotatedEvent {
            epoch: 1,
            signers: map![
                &env,
                (a.clone(), 3),
                (b.clone(), 1),
                (c.clone(), 1),
                (d.clone(), 1)
            ],
            threshold: 3,
            rotation_threshold: 4,
        }]
    );

    // A second change, vetoed by one signer.
    let change2 = client.propose_signer_change(&a, &Vec::new(&env), &vec![&env, d.clone()], &3, &4);
    client.veto_signer_change(&c, &change2);
    rec.record(&env);
    assert_eq!(
        events_named::<SignerChangeVetoedEvent>(&env, at, "signer_change_vetoed"),
        [SignerChangeVetoedEvent { signer: c.clone() }]
    );

    // Upgrade consent, then withdrawn.
    let wasm = id(&env, 0xAB);
    client.approve_upgrade(&b, &wasm);
    client.revoke_upgrade(&b, &wasm);
    rec.record(&env);
    assert_eq!(
        events_named::<UpgradeApprovedEvent>(&env, at, "upgrade_approved"),
        [UpgradeApprovedEvent {
            signer: b.clone(),
            approvals: 1,
            signer_count: 4,
        }]
    );
    assert_eq!(
        events_named::<UpgradeRevokedEvent>(&env, at, "upgrade_revoked"),
        [UpgradeRevokedEvent {
            signer: b,
            approvals: 0,
        }]
    );

    rec.assert_complete();
    let _ = UPGRADE_DELAY;
}
