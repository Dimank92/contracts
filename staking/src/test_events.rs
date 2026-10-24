#![cfg(test)]

//! Event schema v2 (#126): every staking event conforms to events.json
//! and carries the documented data.

extern crate std;

use crate::{
    CarryoverDistributedEvent, InitializedEvent, RewardClaimedEvent, RewardNotifiedEvent,
    StakedEvent, Staking, StakingClient, UnstakeRequestedEvent, WithdrawnEvent, ACC_PRECISION,
    CURRENT_SCHEMA_VERSION,
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
    let new_token = || {
        env.register_stellar_asset_contract_v2(Address::generate(&env))
            .address()
    };
    let (zen, usdc) = (new_token(), new_token());
    let (splitter, user) = (Address::generate(&env), Address::generate(&env));
    token::StellarAssetClient::new(&env, &usdc).mint(&splitter, &10_000);
    token::StellarAssetClient::new(&env, &zen).mint(&user, &1_000);
    let client = StakingClient::new(&env, &env.register_contract(None, Staking));
    let mut rec = Recorder::new("staking", &client.address);

    client.initialize(&zen, &splitter, &100);
    rec.record(&env);
    assert_eq!(
        events_named::<InitializedEvent>(&env, &client.address, "initialized"),
        [InitializedEvent {
            stake_token: zen.clone(),
            splitter: splitter.clone(),
            cooldown: 100,
            schema_version: CURRENT_SCHEMA_VERSION,
        }]
    );

    // Nothing staked yet: the reward is carried over.
    client.notify_reward(&usdc, &400);
    rec.record(&env);
    client.stake(&user, &200);
    rec.record(&env);
    assert_eq!(
        events_named::<CarryoverDistributedEvent>(&env, &client.address, "carryover_distributed"),
        [CarryoverDistributedEvent {
            amount: 400,
            reward_per_token: 2 * ACC_PRECISION,
            total_staked: 200,
        }]
    );
    assert_eq!(
        events_named::<StakedEvent>(&env, &client.address, "staked"),
        [StakedEvent {
            amount: 200,
            stake: 200,
            total_staked: 200,
        }]
    );

    client.notify_reward(&usdc, &100);
    rec.record(&env);
    assert_eq!(
        events_named::<RewardNotifiedEvent>(&env, &client.address, "reward_notified"),
        [
            RewardNotifiedEvent {
                amount: 400,
                reward_per_token: 0,
                undistributed: 400,
                total_staked: 0,
            },
            RewardNotifiedEvent {
                amount: 100,
                reward_per_token: 2 * ACC_PRECISION + ACC_PRECISION / 2,
                undistributed: 0,
                total_staked: 200,
            },
        ]
    );

    client.claim_rewards(&user);
    rec.record(&env);
    assert_eq!(
        events_named::<RewardClaimedEvent>(&env, &client.address, "reward_claimed"),
        [RewardClaimedEvent { amount: 500 }]
    );

    client.request_unstake(&user, &50);
    rec.record(&env);
    assert_eq!(
        events_named::<UnstakeRequestedEvent>(&env, &client.address, "unstake_requested"),
        [UnstakeRequestedEvent {
            amount: 50,
            stake: 150,
            pending: 50,
            unlock_at: 100,
            total_staked: 150,
        }]
    );

    env.ledger().set_timestamp(100);
    client.withdraw(&user);
    rec.record(&env);
    assert_eq!(
        events_named::<WithdrawnEvent>(&env, &client.address, "withdrawn"),
        [WithdrawnEvent { amount: 50 }]
    );

    rec.assert_complete();
}
