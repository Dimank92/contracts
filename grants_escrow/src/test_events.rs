#![cfg(test)]

//! Event schema v2 (#126): every grants_escrow event conforms to
//! events.json and carries the documented data.

extern crate std;

use crate::{
    FundsReclaimedEvent, GrantCreatedEvent, GrantsEscrow, GrantsEscrowClient,
    MilestoneApprovedEvent, MilestoneReleasedEvent, MilestoneSubmittedEvent, GRACE_PERIOD,
};
use event_spec::{events_named, Recorder};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, vec, Address, BytesN, Env,
};

#[test]
fn every_event_conforms_to_the_spec() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let client = GrantsEscrowClient::new(&env, &env.register_contract(None, GrantsEscrow));
    let mut rec = Recorder::new("grants_escrow", &client.address);
    let (funder, grantee) = (Address::generate(&env), Address::generate(&env));
    let (r1, r2) = (Address::generate(&env), Address::generate(&env));
    token::StellarAssetClient::new(&env, &token).mint(&funder, &10_000);

    let id = client.create_grant(
        &funder,
        &grantee,
        &token,
        &vec![&env, (1_000i128, 2_000u64), (500i128, 3_000u64)],
        &vec![&env, r1.clone(), r2.clone()],
        &2,
    );
    rec.record(&env);
    assert_eq!(
        events_named::<GrantCreatedEvent>(&env, &client.address, "grant_created"),
        [GrantCreatedEvent {
            funder: funder.clone(),
            grantee: grantee.clone(),
            token: token.clone(),
            total: 1_500,
            reviewers: vec![&env, r1.clone(), r2.clone()],
            quorum: 2,
            amounts: vec![&env, 1_000, 500],
            deadlines: vec![&env, 2_000, 3_000],
            created_at: 1_000,
        }]
    );

    let evidence = BytesN::from_array(&env, &[9; 32]);
    client.submit_milestone(&id, &0, &evidence);
    rec.record(&env);
    assert_eq!(
        events_named::<MilestoneSubmittedEvent>(&env, &client.address, "milestone_submitted"),
        [MilestoneSubmittedEvent {
            evidence_hash: evidence,
            submitted_at: 1_000,
        }]
    );

    client.approve_milestone(&id, &0, &r1);
    rec.record(&env);
    client.approve_milestone(&id, &0, &r2);
    rec.record(&env);
    assert_eq!(
        events_named::<MilestoneApprovedEvent>(&env, &client.address, "milestone_approved"),
        [
            MilestoneApprovedEvent {
                reviewer: r1,
                approvals: 1,
                quorum: 2,
            },
            MilestoneApprovedEvent {
                reviewer: r2,
                approvals: 2,
                quorum: 2,
            },
        ]
    );
    assert_eq!(
        events_named::<MilestoneReleasedEvent>(&env, &client.address, "milestone_released"),
        [MilestoneReleasedEvent {
            grantee,
            amount: 1_000,
        }]
    );

    env.ledger().set_timestamp(3_000 + GRACE_PERIOD + 1);
    client.reclaim(&id);
    rec.record(&env);
    assert_eq!(
        events_named::<FundsReclaimedEvent>(&env, &client.address, "funds_reclaimed"),
        [FundsReclaimedEvent {
            funder,
            amount: 500,
            milestones: vec![&env, 1],
        }]
    );

    rec.assert_complete();
}
