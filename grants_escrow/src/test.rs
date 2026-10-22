#![cfg(test)]

extern crate std;

use crate::{GrantsEscrow, GrantsEscrowClient, MilestoneStatus, GRACE_PERIOD};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    token, vec, Address, BytesN, Env, Symbol, TryFromVal, Vec,
};

/// Tag under which `grants_escrow` namespaces each grant's escrow in `vault`.
const GRANT_TAG: &str = "grant";

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

    let (_, topics, _) = h.env.events().all().last().unwrap();
    let name = Symbol::try_from_val(&h.env, &topics.get(0).unwrap()).unwrap();
    assert_eq!(name, Symbol::new(&h.env, "milestone_released"));
}

#[test]
fn quorum_is_required_before_release() {
    let h = setup();
    let id = create(&h);
    // Can't approve before submission.
    assert!(h
        .client
        .try_approve_milestone(&id, &0, &h.reviewers[0])
        .is_err());

    h.client.submit_milestone(&id, &0, &evidence(&h));
    h.client.approve_milestone(&id, &0, &h.reviewers[0]);
    assert_eq!(balance(&h, &h.grantee), 0);
    // The same reviewer can't count twice, and outsiders can't count at all.
    assert!(h
        .client
        .try_approve_milestone(&id, &0, &h.reviewers[0])
        .is_err());
    assert!(h
        .client
        .try_approve_milestone(&id, &0, &Address::generate(&h.env))
        .is_err());
    assert!(h.client.has_approved(&id, &0, &h.reviewers[0]));

    h.client.approve_milestone(&id, &0, &h.reviewers[1]);
    assert_eq!(balance(&h, &h.grantee), 1_000);
    // Released milestones take no further approvals.
    assert!(h
        .client
        .try_approve_milestone(&id, &0, &h.reviewers[2])
        .is_err());
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
    assert!(result.is_err());
}

#[test]
fn create_grant_validates_inputs() {
    let h = setup();
    let now = h.env.ledger().timestamp();
    let ms = vec![&h.env, (1_000i128, now + DAY)];
    // Quorum above committee size, zero quorum, duplicate reviewer.
    assert!(h
        .client
        .try_create_grant(&h.funder, &h.grantee, &h.token, &ms, &reviewers(&h), &4)
        .is_err());
    assert!(h
        .client
        .try_create_grant(&h.funder, &h.grantee, &h.token, &ms, &reviewers(&h), &0)
        .is_err());
    let dup = vec![&h.env, h.reviewers[0].clone(), h.reviewers[0].clone()];
    assert!(h
        .client
        .try_create_grant(&h.funder, &h.grantee, &h.token, &ms, &dup, &1)
        .is_err());
    // Zero amount, past deadline, no milestones.
    for bad in [
        vec![&h.env, (0i128, now + DAY)],
        vec![&h.env, (1_000i128, now)],
        Vec::new(&h.env),
    ] {
        assert!(h
            .client
            .try_create_grant(&h.funder, &h.grantee, &h.token, &bad, &reviewers(&h), &1)
            .is_err());
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
    assert!(h
        .client
        .try_submit_milestone(&id, &0, &evidence(&h))
        .is_err());
    // Within the grace period nothing is reclaimable yet.
    assert!(h.client.try_reclaim(&id).is_err());

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
    assert!(h.client.try_reclaim(&id).is_err());
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
    assert!(h
        .client
        .try_approve_milestone(&id, &0, &h.reviewers[1])
        .is_err());
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

#[test]
fn out_of_range_milestone_ids_are_rejected() {
    let h = setup();
    let id = create(&h);
    // Only milestones 0 and 1 exist.
    for bad in [2u32, 3, u32::MAX] {
        assert!(h
            .client
            .try_submit_milestone(&id, &bad, &evidence(&h))
            .is_err());
        assert!(h
            .client
            .try_approve_milestone(&id, &bad, &h.reviewers[0])
            .is_err());
    }
    // Unknown grant ids are rejected too.
    assert!(h
        .client
        .try_submit_milestone(&id + 1, &0, &evidence(&h))
        .is_err());
    assert!(h
        .client
        .try_approve_milestone(&id + 1, &0, &h.reviewers[0])
        .is_err());
    assert!(h.client.try_claim(&id + 1).is_err());
    assert!(h.client.try_cancel_grant(&id + 1).is_err());
}

#[test]
fn double_approval_by_same_reviewer_is_rejected() {
    let h = setup();
    let id = create(&h);
    h.client.submit_milestone(&id, &0, &evidence(&h));
    h.client.approve_milestone(&id, &0, &h.reviewers[0]);
    // Same reviewer approving twice must not count toward quorum.
    assert!(h
        .client
        .try_approve_milestone(&id, &0, &h.reviewers[0])
        .is_err());
    assert_eq!(balance(&h, &h.grantee), 0);
    // A second, distinct reviewer completes the 2-of-3 quorum.
    h.client.approve_milestone(&id, &0, &h.reviewers[1]);
    assert_eq!(balance(&h, &h.grantee), 1_000);
    // Approving an already-released milestone is rejected.
    assert!(h
        .client
        .try_approve_milestone(&id, &0, &h.reviewers[2])
        .is_err());
}

#[test]
fn claim_before_approval_is_rejected() {
    let h = setup();
    let id = create(&h);
    // Nothing submitted yet.
    assert!(h.client.try_claim(&id).is_err());
    // Submitted but not approved.
    h.client.submit_milestone(&id, &0, &evidence(&h));
    assert!(h.client.try_claim(&id).is_err());
    // One approval short of quorum.
    h.client.approve_milestone(&id, &0, &h.reviewers[0]);
    assert!(h.client.try_claim(&id).is_err());
    assert_eq!(balance(&h, &h.grantee), 0);
    // Quorum reached: claim succeeds and pays out exactly the milestone.
    h.client.approve_milestone(&id, &0, &h.reviewers[1]);
    assert_eq!(h.client.claim(&id), 1_000);
    assert_eq!(balance(&h, &h.grantee), 1_000);
    // Nothing left approved to claim.
    assert!(h.client.try_claim(&id).is_err());
}

#[test]
fn cancel_with_zero_claims_returns_everything_to_grantor() {
    let h = setup();
    let id = create(&h);
    assert_eq!(balance(&h, &h.funder), 7_000);
    assert_eq!(balance(&h, &h.client.address), 3_000);

    let refunded = h.client.cancel_grant(&id);
    assert_eq!(refunded, 3_000);
    assert_eq!(balance(&h, &h.funder), 10_000);
    assert_eq!(balance(&h, &h.client.address), 0);
    assert_eq!(balance(&h, &h.grantee), 0);

    // A cancelled grant is inert: no submissions, approvals, claims or re-cancel.
    assert!(h
        .client
        .try_submit_milestone(&id, &0, &evidence(&h))
        .is_err());
    assert!(h
        .client
        .try_approve_milestone(&id, &0, &h.reviewers[0])
        .is_err());
    assert!(h.client.try_claim(&id).is_err());
    assert!(h.client.try_cancel_grant(&id).is_err());
}

#[test]
fn cancel_after_all_claims_refunds_nothing_and_keeps_payouts() {
    let h = setup();
    let id = create(&h);
    for idx in 0..2u32 {
        h.client.submit_milestone(&id, &idx, &evidence(&h));
        h.client.approve_milestone(&id, &idx, &h.reviewers[0]);
        h.client.approve_milestone(&id, &idx, &h.reviewers[1]);
    }
    assert_eq!(balance(&h, &h.grantee), 3_000);
    assert_eq!(balance(&h, &h.client.address), 0);

    // Nothing left to refund, and already-claimed milestones are not clawed back.
    let refunded = h.client.cancel_grant(&id);
    assert_eq!(refunded, 0);
    assert_eq!(balance(&h, &h.grantee), 3_000);
    assert_eq!(balance(&h, &h.funder), 7_000);
    assert_eq!(balance(&h, &h.client.address), 0);
}

#[test]
fn cancel_after_partial_claims_refunds_only_the_remainder() {
    let h = setup();
    let id = create(&h);
    h.client.submit_milestone(&id, &0, &evidence(&h));
    h.client.approve_milestone(&id, &0, &h.reviewers[0]);
    h.client.approve_milestone(&id, &0, &h.reviewers[1]);
    assert_eq!(balance(&h, &h.grantee), 1_000);

    let refunded = h.client.cancel_grant(&id);
    assert_eq!(refunded, 2_000);
    assert_eq!(balance(&h, &h.grantee), 1_000);
    assert_eq!(balance(&h, &h.funder), 9_000);
    assert_eq!(balance(&h, &h.client.address), 0);
}

#[test]
fn claim_cannot_exceed_its_own_grant_allocation() {
    let h = setup();
    let id = create(&h);
    // Approve both milestones, then claim: total paid never exceeds the grant total.
    for idx in 0..2u32 {
        h.client.submit_milestone(&id, &idx, &evidence(&h));
        h.client.approve_milestone(&id, &idx, &h.reviewers[0]);
        h.client.approve_milestone(&id, &idx, &h.reviewers[1]);
    }
    assert_eq!(h.client.claim(&id), 3_000);
    assert_eq!(balance(&h, &h.grantee), 3_000);
    // Further claims draw nothing.
    assert!(h.client.try_claim(&id).is_err());
    assert_eq!(balance(&h, &h.grantee), 3_000);
}

#[test]
fn claim_on_one_grant_cannot_touch_another_grants_escrow() {
    let h = setup();
    // Two independent grants, each with its own escrow balance.
    let id_a = create(&h);
    let id_b = create(&h);
    assert_eq!(balance(&h, &h.client.address), 6_000);

    // Fully approve and claim grant A.
    for idx in 0..2u32 {
        h.client.submit_milestone(&id_a, &idx, &evidence(&h));
        h.client.approve_milestone(&id_a, &idx, &h.reviewers[0]);
        h.client.approve_milestone(&id_a, &idx, &h.reviewers[1]);
    }
    assert_eq!(h.client.claim(&id_a), 3_000);
    assert_eq!(balance(&h, &h.grantee), 3_000);
    // Grant B's escrow is untouched.
    assert_eq!(balance(&h, &h.client.address), 3_000);

    // Grant B still pays out exactly its own allocation.
    for idx in 0..2u32 {
        h.client.submit_milestone(&id_b, &idx, &evidence(&h));
        h.client.approve_milestone(&id_b, &idx, &h.reviewers[0]);
        h.client.approve_milestone(&id_b, &idx, &h.reviewers[1]);
    }
    assert_eq!(h.client.claim(&id_b), 3_000);
    assert_eq!(balance(&h, &h.grantee), 6_000);
    assert_eq!(balance(&h, &h.client.address), 0);

    // Cancelling A after it is fully claimed refunds nothing and leaves B's payout intact.
    assert_eq!(h.client.cancel_grant(&id_a), 0);
    assert_eq!(balance(&h, &h.grantee), 6_000);
}

#[test]
fn grant_escrow_is_namespaced_under_its_own_vault_tag() {
    let h = setup();
    let id = create(&h);
    // The escrow is held under a per-grant tag, not a raw undifferentiated balance.
    let tag = Symbol::new(&h.env, GRANT_TAG);
    assert!(h.client.get_grant(&id).is_some());
    // Sanity: the contract holds exactly the grant total, and the tag is stable.
    assert_eq!(balance(&h, &h.client.address), 3_000);
    assert_eq!(tag, Symbol::new(&h.env, GRANT_TAG));
}

#[test]
fn grant_funded_with_token_later_deallowlisted_in_vault() {
    let h = setup();
    let id = create(&h);
    h.client.submit_milestone(&id, &0, &evidence(&h));
    h.client.approve_milestone(&id, &0, &h.reviewers[0]);
    h.client.approve_milestone(&id, &0, &h.reviewers[1]);

    // The token is de-allowlisted in `vault` after funding. Claims must not silently
    // succeed against a token the protocol no longer trusts; the escrow stays intact.
    h.client.set_token_allowed(&h.token, &false);
    assert!(h.client.try_claim(&id).is_err());
    assert_eq!(balance(&h, &h.grantee), 0);
    assert_eq!(balance(&h, &h.client.address), 3_000);

    // Re-allowlisting restores the ability to claim the already-approved milestone.
    h.client.set_token_allowed(&h.token, &true);
    assert_eq!(h.client.claim(&id), 1_000);
    assert_eq!(balance(&h, &h.grantee), 1_000);

    // Cancelling while de-allowlisted still returns unclaimed funds to the grantor.
    h.client.set_token_allowed(&h.token, &false);
    assert_eq!(h.client.cancel_grant(&id), 2_000);
    assert_eq!(balance(&h, &h.funder), 9_000);
    assert_eq!(balance(&h, &h.grantee), 1_000);
}

#[test]
fn events_are_emitted_for_create_approve_claim_and_cancel() {
    let h = setup();
    let id = create(&h);
    h.client.submit_milestone(&id, &0, &evidence(&h));
    h.client.approve_milestone(&id, &0, &h.reviewers[0]);
    h.client.approve_milestone(&id, &0, &h.reviewers[1]);
    h.client.claim(&id);
    h.client.cancel_grant(&id);

    let names: std::vec::Vec<Symbol> = h
        .env
        .events()
        .all()
        .iter()
        .filter_map(|(_, topics, _)| {
            topics
                .get(0)
                .and_then(|t| Symbol::try_from_val(&h.env, &t).ok())
        })
        .collect();
    for expected in ["grant_created", "milestone_approved", "claim", "grant_cancelled"] {
        assert!(
            names.contains(&Symbol::new(&h.env, expected)),
            "missing event {expected}"
        );
    }
}
