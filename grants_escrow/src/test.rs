#![cfg(test)]

extern crate std;

use crate::{Error, GrantsEscrow, GrantsEscrowClient, MilestoneStatus, GRACE_PERIOD};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, vec, Address, BytesN, Env, Symbol, TryFromVal, Vec,
};

const DAY: u64 = 86_400;

struct Harness<'a> {
    env: Env,
    client: GrantsEscrowClient<'a>,
    token: Address,
    funder: Address,
    grantee: Address,
    reviewers: [Address; 3],
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let client = GrantsEscrowClient::new(&env, &env.register_contract(None, GrantsEscrow));
    let funder = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token).mint(&funder, &10_000);
    Harness {
        grantee: Address::generate(&env),
        reviewers: [
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        ],
        env,
        client,
        token,
        funder,
    }
}

fn balance(h: &Harness, of: &Address) -> i128 {
    token::Client::new(&h.env, &h.token).balance(of)
}

fn reviewers(h: &Harness) -> Vec<Address> {
    vec![
        &h.env,
        h.reviewers[0].clone(),
        h.reviewers[1].clone(),
        h.reviewers[2].clone(),
    ]
}

/// Two milestones: 1_000 due in 10 days, 2_000 due in 20 days; 2-of-3.
fn create(h: &Harness) -> u64 {
    let now = h.env.ledger().timestamp();
    h.client.create_grant(
        &h.funder,
        &h.grantee,
        &h.token,
        &vec![
            &h.env,
            (1_000i128, now + 10 * DAY),
            (2_000i128, now + 20 * DAY),
        ],
        &reviewers(h),
        &2,
    )
}

fn evidence(h: &Harness) -> BytesN<32> {
    BytesN::from_array(&h.env, &[7; 32])
}

#[test]
fn full_grant_lifecycle_releases_every_milestone() {
    let h = setup();
    let id = create(&h);
    assert_eq!(balance(&h, &h.client.address), 3_000);
    assert_eq!(balance(&h, &h.funder), 7_000);

    for idx in 0..2u32 {
        h.client.submit_milestone(&id, &idx, &evidence(&h));
        h.client.approve_milestone(&id, &idx, &h.reviewers[0]);
        h.client.approve_milestone(&id, &idx, &h.reviewers[2]);
    }

    assert_eq!(balance(&h, &h.grantee), 3_000);
    assert_eq!(balance(&h, &h.client.address), 0);
    let grant = h.client.get_grant(&id).unwrap();
    for m in grant.milestones.iter() {
        assert_eq!(m.status, MilestoneStatus::Released);
        assert_eq!(m.evidence_hash, evidence(&h));
    }

    let (topics, _) = event_spec::last_event(&h.env, &h.client.address);
    let name = Symbol::try_from_val(&h.env, &topics.get(0).unwrap()).unwrap();
    assert_eq!(name, Symbol::new(&h.env, "milestone_released"));
}

#[test]
fn quorum_is_required_before_release() {
    let h = setup();
    let id = create(&h);
    // Can't approve before submission.
    assert_eq!(
        h.client.try_approve_milestone(&id, &0, &h.reviewers[0]),
        Err(Ok(Error::InvalidMilestoneState))
    );

    h.client.submit_milestone(&id, &0, &evidence(&h));
    h.client.approve_milestone(&id, &0, &h.reviewers[0]);
    assert_eq!(balance(&h, &h.grantee), 0);
    // The same reviewer can't count twice, and outsiders can't count at all.
    assert_eq!(
        h.client.try_approve_milestone(&id, &0, &h.reviewers[0]),
        Err(Ok(Error::AlreadyApproved))
    );
    assert_eq!(
        h.client
            .try_approve_milestone(&id, &0, &Address::generate(&h.env)),
        Err(Ok(Error::NotAReviewer))
    );
    assert!(h.client.has_approved(&id, &0, &h.reviewers[0]));

    h.client.approve_milestone(&id, &0, &h.reviewers[1]);
    assert_eq!(balance(&h, &h.grantee), 1_000);
    // Released milestones take no further approvals.
    assert_eq!(
        h.client.try_approve_milestone(&id, &0, &h.reviewers[2]),
        Err(Ok(Error::InvalidMilestoneState))
    );
}

#[test]
fn reviewer_cannot_be_the_grantee() {
    let h = setup();
    let now = h.env.ledger().timestamp();
    let result = h.client.try_create_grant(
        &h.funder,
        &h.grantee,
        &h.token,
        &vec![&h.env, (1_000i128, now + DAY)],
        &vec![&h.env, h.reviewers[0].clone(), h.grantee.clone()],
        &1,
    );
    assert_eq!(result, Err(Ok(Error::ReviewerIsGrantee)));
}

#[test]
fn create_grant_validates_inputs() {
    let h = setup();
    let now = h.env.ledger().timestamp();
    let ms = vec![&h.env, (1_000i128, now + DAY)];
    // Quorum above committee size, zero quorum, duplicate reviewer.
    assert_eq!(
        h.client
            .try_create_grant(&h.funder, &h.grantee, &h.token, &ms, &reviewers(&h), &4),
        Err(Ok(Error::InvalidQuorum))
    );
    assert_eq!(
        h.client
            .try_create_grant(&h.funder, &h.grantee, &h.token, &ms, &reviewers(&h), &0),
        Err(Ok(Error::InvalidQuorum))
    );
    let dup = vec![&h.env, h.reviewers[0].clone(), h.reviewers[0].clone()];
    assert_eq!(
        h.client
            .try_create_grant(&h.funder, &h.grantee, &h.token, &ms, &dup, &1),
        Err(Ok(Error::InvalidReviewers))
    );
    // Zero amount, past deadline, no milestones.
    for bad in [
        vec![&h.env, (0i128, now + DAY)],
        vec![&h.env, (1_000i128, now)],
        Vec::new(&h.env),
    ] {
        assert_eq!(
            h.client
                .try_create_grant(&h.funder, &h.grantee, &h.token, &bad, &reviewers(&h), &1),
            Err(Ok(Error::InvalidMilestones))
        );
    }
}

#[test]
fn missed_deadline_is_reclaimable_only_after_grace() {
    let h = setup();
    let id = create(&h);
    let deadline = h
        .client
        .get_grant(&id)
        .unwrap()
        .milestones
        .get(0)
        .unwrap()
        .deadline;

    h.env.ledger().with_mut(|l| l.timestamp = deadline + 1);
    assert_eq!(
        h.client.try_submit_milestone(&id, &0, &evidence(&h)),
        Err(Ok(Error::DeadlinePassed))
    );
    // Within the grace period nothing is reclaimable yet.
    assert_eq!(h.client.try_reclaim(&id), Err(Ok(Error::NothingToReclaim)));

    h.env
        .ledger()
        .with_mut(|l| l.timestamp = deadline + GRACE_PERIOD + 1);
    assert_eq!(h.client.reclaim(&id), 1_000);
    assert_eq!(balance(&h, &h.funder), 8_000);

    // The second milestone is still live for the grantee.
    h.client.submit_milestone(&id, &1, &evidence(&h));
    h.client.approve_milestone(&id, &1, &h.reviewers[0]);
    h.client.approve_milestone(&id, &1, &h.reviewers[1]);
    assert_eq!(balance(&h, &h.grantee), 2_000);
    assert_eq!(h.client.try_reclaim(&id), Err(Ok(Error::NothingToReclaim)));
}

#[test]
fn submitted_but_unapproved_milestone_is_reclaimable_after_grace() {
    let h = setup();
    let id = create(&h);
    h.client.submit_milestone(&id, &0, &evidence(&h));
    h.client.approve_milestone(&id, &0, &h.reviewers[0]);

    let deadline = h
        .client
        .get_grant(&id)
        .unwrap()
        .milestones
        .get(0)
        .unwrap()
        .deadline;
    h.env
        .ledger()
        .with_mut(|l| l.timestamp = deadline + GRACE_PERIOD + 1);
    assert_eq!(h.client.reclaim(&id), 1_000);
    assert_eq!(
        h.client.try_approve_milestone(&id, &0, &h.reviewers[1]),
        Err(Ok(Error::InvalidMilestoneState))
    );
}

#[test]
fn grants_are_paginated_per_grantee() {
    let h = setup();
    let ids: std::vec::Vec<u64> = (0..3).map(|_| create(&h)).collect();
    assert_eq!(h.client.get_grantee_grant_count(&h.grantee), 3);
    assert_eq!(
        h.client.get_grants_by_grantee(&h.grantee, &0, &2),
        vec![&h.env, ids[0], ids[1]]
    );
    assert_eq!(
        h.client.get_grants_by_grantee(&h.grantee, &2, &2),
        vec![&h.env, ids[2]]
    );
    assert_eq!(
        h.client.get_grants_by_grantee(&h.grantee, &5, &2),
        Vec::<u64>::new(&h.env)
    );
    assert_eq!(h.client.get_grant_count(), 3);
}
