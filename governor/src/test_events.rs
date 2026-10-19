#![cfg(test)]

//! Event schema v2 (#126): every governor event conforms to events.json
//! and carries the documented data.

extern crate std;

use crate::test::MockVotesClient;
use crate::test_timelock::{MockTimelock, MockTimelockClient};
use crate::{
    Governor, GovernorClient, InitializedEvent, ParamSetEvent, ProposalCanceledEvent,
    ProposalCreatedEvent, ProposalQueuedEvent, VoteCastEvent, CURRENT_SCHEMA_VERSION,
};
use event_spec::{events_named, Recorder};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    vec, Address, BytesN, Env, IntoVal, Symbol,
};

#[test]
fn every_event_conforms_to_the_spec() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_sequence_number(100);
    env.ledger().set_timestamp(1_000);
    let votes = MockVotesClient::new(&env, &env.register_contract(None, crate::test::MockVotes));
    let gov_id = env.register_contract(None, Governor);
    let tl = MockTimelockClient::new(&env, &env.register_contract(None, MockTimelock));
    tl.initialize(&gov_id, &Address::generate(&env), &86_400, &86_400);
    let gov = GovernorClient::new(&env, &gov_id);
    let mut rec = Recorder::new("governor", &gov.address);

    gov.initialize(&votes.address, &tl.address, &10, &720, &400, &100);
    rec.record(&env);
    assert_eq!(
        events_named::<InitializedEvent>(&env, &gov.address, "initialized"),
        [InitializedEvent {
            token: votes.address.clone(),
            timelock: tl.address.clone(),
            voting_delay: 10,
            voting_period: 720,
            quorum_bps: 400,
            proposal_threshold: 100,
            schema_version: CURRENT_SCHEMA_VERSION,
        }]
    );
    assert_eq!(
        events_named::<ParamSetEvent>(&env, &gov.address, "param_set"),
        [
            ParamSetEvent { value: 10 },
            ParamSetEvent { value: 720 },
            ParamSetEvent { value: 400 },
            ParamSetEvent { value: 100 },
        ]
    );

    let (alice, bob) = (Address::generate(&env), Address::generate(&env));
    votes.adjust(&alice, &1_000);
    votes.adjust(&bob, &300);
    env.ledger().set_sequence_number(101);

    let targets = vec![&env, gov.address.clone()];
    let fns = vec![&env, Symbol::new(&env, "set_quorum_bps")];
    let args = vec![&env, vec![&env, 500u32.into_val(&env)]];
    let desc = BytesN::from_array(&env, &[7; 32]);
    let id = gov.propose(&alice, &targets, &fns, &args, &desc);
    rec.record(&env);
    let created = events_named::<ProposalCreatedEvent>(&env, &gov.address, "proposal_created");
    assert_eq!(created.len(), 1);
    let c = &created[0];
    assert_eq!(
        (&c.proposer, &c.targets, &c.fns, &c.description_hash),
        (&alice, &targets, &fns, &desc)
    );
    assert_eq!(c.args.len(), 1);
    assert_eq!((c.snapshot, c.deadline), (111, 831));

    env.ledger().set_sequence_number(112);
    gov.cast_vote(&alice, &id, &1);
    rec.record(&env);
    gov.cast_vote(&bob, &id, &0);
    rec.record(&env);
    assert_eq!(
        events_named::<VoteCastEvent>(&env, &gov.address, "vote_cast"),
        [
            VoteCastEvent {
                support: 1,
                weight: 1_000,
                for_votes: 1_000,
                against_votes: 0,
                abstain_votes: 0,
            },
            VoteCastEvent {
                support: 0,
                weight: 300,
                for_votes: 1_000,
                against_votes: 300,
                abstain_votes: 0,
            },
        ]
    );

    env.ledger().set_sequence_number(832);
    gov.queue(&id);
    rec.record(&env);
    assert_eq!(
        events_named::<ProposalQueuedEvent>(&env, &gov.address, "proposal_queued"),
        [ProposalQueuedEvent {
            eta: 1_000 + 86_400,
            // 4% of the 1_300 supply at the snapshot.
            quorum: 52,
        }]
    );

    // The proposer drops below the threshold: anyone may cancel.
    votes.adjust(&alice, &-1_000);
    env.ledger().set_sequence_number(833);
    gov.cancel(&bob, &id);
    rec.record(&env);
    assert_eq!(
        events_named::<ProposalCanceledEvent>(&env, &gov.address, "proposal_canceled"),
        [ProposalCanceledEvent {
            caller: bob,
            was_queued: true,
        }]
    );

    rec.assert_complete();
}
