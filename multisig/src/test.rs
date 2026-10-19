#![cfg(test)]

extern crate std;

use crate::types::ActionStatus;
use crate::{Error, Multisig, MultisigClient, MAX_PAGE_LIMIT, MAX_PENDING_PER_SIGNER};
use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::{Address as _, Events as _, Ledger as _},
    vec, Address, BytesN, Env, IntoVal, Symbol, TryFromVal, Val, Vec,
};

struct Harness<'a> {
    env: Env,
    client: MultisigClient<'a>,
    signers: [Address; 3],
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let signers = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(
        &vec![
            &env,
            signers[0].clone(),
            signers[1].clone(),
            signers[2].clone(),
        ],
        &2,
        &0, // no approval expiry
    );

    Harness {
        env,
        client,
        signers,
    }
}

#[test]
fn initialize_sets_signers_and_threshold() {
    let h = setup();
    assert_eq!(h.client.get_signer_count(), 3);
    assert_eq!(h.client.get_threshold(), 2);
    for signer in &h.signers {
        assert!(h.client.is_signer(signer));
    }
    let stranger = Address::generate(&h.env);
    assert!(!h.client.is_signer(&stranger));
}

#[test]
fn initialize_twice_panics() {
    let h = setup();
    assert_eq!(
        h.client
            .try_initialize(&vec![&h.env, h.signers[0].clone()], &1, &0),
        Err(Ok(Error::AlreadyInitialized))
    );
}

#[test]
fn initialize_rejects_a_zero_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let signers = vec![&env, Address::generate(&env)];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    assert_eq!(
        client.try_initialize(&signers, &0, &0),
        Err(Ok(Error::InvalidThreshold))
    );
}

#[test]
fn initialize_rejects_a_threshold_above_the_signer_count() {
    let env = Env::default();
    env.mock_all_auths();
    let signers = vec![&env, Address::generate(&env), Address::generate(&env)];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    assert_eq!(
        client.try_initialize(&signers, &3, &0),
        Err(Ok(Error::InvalidThreshold))
    );
}

#[test]
fn initialize_rejects_a_duplicate_signer() {
    let env = Env::default();
    env.mock_all_auths();
    let signer = Address::generate(&env);
    let signers = vec![&env, signer.clone(), signer];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    assert_eq!(
        client.try_initialize(&signers, &1, &0),
        Err(Ok(Error::DuplicateSigner))
    );
}

// ─── approve / revoke / is_approved ────────────────────────────────────────

#[test]
fn approve_records_the_signers_own_vote() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
}

#[test]
fn is_approved_flips_true_once_the_threshold_is_reached() {
    let h = setup();
    // Threshold is 2 of 3.
    assert!(!h.client.is_approved(&aid(&h.env, 42)));

    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    assert!(!h.client.is_approved(&aid(&h.env, 42)));

    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    assert!(h.client.is_approved(&aid(&h.env, 42)));
}

#[test]
fn different_action_ids_track_approvals_independently() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.client.approve(&h.signers[0], &aid(&h.env, 2));
    h.client.approve(&h.signers[1], &aid(&h.env, 2));

    assert!(!h.client.is_approved(&aid(&h.env, 1))); // only 1 of 3
    assert!(h.client.is_approved(&aid(&h.env, 2))); // 2 of 3
}

#[test]
fn approve_rejects_a_non_signer() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    assert_eq!(
        h.client.try_approve(&stranger, &aid(&h.env, 42)),
        Err(Ok(Error::NotASigner))
    );
}

#[test]
fn approve_rejects_the_same_signer_voting_twice() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    assert_eq!(
        h.client.try_approve(&h.signers[0], &aid(&h.env, 42)),
        Err(Ok(Error::AlreadyApproved))
    );
}

#[test]
fn revoke_withdraws_the_signers_vote() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    assert!(h.client.is_approved(&aid(&h.env, 42)));

    h.client.revoke(&h.signers[1], &aid(&h.env, 42));
    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
    // Dropping below threshold un-approves the action.
    assert!(!h.client.is_approved(&aid(&h.env, 42)));
}

#[test]
fn a_revoked_signer_can_approve_again() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.revoke(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
}

#[test]
fn revoke_rejects_a_signer_who_never_approved() {
    let h = setup();
    assert_eq!(
        h.client.try_revoke(&h.signers[0], &aid(&h.env, 42)),
        Err(Ok(Error::NotYetApproved))
    );
}

// ─── approval expiry ────────────────────────────────────────────────────────

fn setup_with_ttl<'a>(ttl: u64) -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let signers = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(
        &vec![
            &env,
            signers[0].clone(),
            signers[1].clone(),
            signers[2].clone(),
        ],
        &2,
        &ttl,
    );

    Harness {
        env,
        client,
        signers,
    }
}

#[test]
fn zero_ttl_means_approvals_never_expire() {
    let h = setup_with_ttl(0);
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 1_000_000_000);
    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
}

#[test]
fn approval_older_than_ttl_stops_counting() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 3601);
    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 0);
}

#[test]
fn is_approved_drops_once_enough_approvals_expire() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    assert!(h.client.is_approved(&aid(&h.env, 42)));

    // signers[1] approves again later, refreshing only its own timestamp.
    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 1800);
    h.client.revoke(&h.signers[1], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));

    // Advance past signers[0]'s original approval but not signers[1]'s
    // refreshed one.
    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 1900);
    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
    assert!(!h.client.is_approved(&aid(&h.env, 42)));
}

#[test]
fn a_signer_can_reapprove_after_their_own_approval_expires() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 3601);
    // Expired, not merely revoked — re-approving must not panic with
    // AlreadyApproved.
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
}

#[test]
fn revoke_rejects_an_already_expired_approval() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 3601);
    assert_eq!(
        h.client.try_revoke(&h.signers[0], &aid(&h.env, 42)),
        Err(Ok(Error::NotYetApproved))
    );
}

#[test]
fn get_approval_ttl_returns_the_configured_value() {
    let h = setup_with_ttl(7200);
    assert_eq!(h.client.get_approval_ttl(), 7200);
}

// ─── reset ──────────────────────────────────────────────────────────────────

#[test]
fn reset_clears_every_signers_approval() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    assert!(h.client.is_approved(&aid(&h.env, 42)));

    h.client.reset(&aid(&h.env, 42));

    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 0);
    assert!(!h.client.is_approved(&aid(&h.env, 42)));
}

#[test]
fn action_id_can_be_approved_fresh_after_a_reset() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    h.client.reset(&aid(&h.env, 42));

    // The same id, reused for a later action, starts from a clean slate —
    // a single signer's leftover vote can't carry over.
    h.client.approve(&h.signers[2], &aid(&h.env, 42));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
    assert!(!h.client.is_approved(&aid(&h.env, 42)));
}

#[test]
fn reset_does_not_affect_other_action_ids() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.client.approve(&h.signers[1], &aid(&h.env, 1));
    h.client.approve(&h.signers[0], &aid(&h.env, 2));
    h.client.approve(&h.signers[1], &aid(&h.env, 2));

    h.client.reset(&aid(&h.env, 1));

    assert!(!h.client.is_approved(&aid(&h.env, 1)));
    assert!(h.client.is_approved(&aid(&h.env, 2)));
}

#[test]
fn reset_rejects_an_action_below_threshold() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42)); // only 1 of 3, threshold is 2
    assert_eq!(
        h.client.try_reset(&aid(&h.env, 42)),
        Err(Ok(Error::NotYetApproved))
    );
}

// ─── events ──────────────────────────────────────────────────────────────────

#[test]
fn approve_emits_an_approved_event() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    let events = h.env.events().all();
    let (contract_id, _topics, _data) = events.last().unwrap();
    assert_eq!(contract_id, h.client.address);
}

#[test]
fn approved_event_topics_carry_the_action_id_and_signer() {
    // Schema v2: (name, version, action_id, signer). Checking
    // contract_id alone proves nothing about WHICH signer or action_id
    // was recorded, so decode both.
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    let events = h.env.events().all();
    let (_, topics, _data) = events.last().unwrap();
    assert_eq!(
        u32::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap(),
        2
    );
    let action_id = BytesN::<32>::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    let signer = Address::try_from_val(&h.env, &topics.get(3).unwrap()).unwrap();
    assert_eq!(signer, h.signers[0]);
    assert_eq!(action_id, aid(&h.env, 42));
}

#[test]
fn revoke_emits_a_revoked_event() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.revoke(&h.signers[0], &aid(&h.env, 42));

    let events = h.env.events().all();
    let (contract_id, _topics, _data) = events.last().unwrap();
    assert_eq!(contract_id, h.client.address);
}

#[test]
fn revoked_event_topics_carry_the_signer_and_action_id() {
    let h = setup();
    h.client.approve(&h.signers[1], &aid(&h.env, 7));
    h.client.revoke(&h.signers[1], &aid(&h.env, 7));

    let events = h.env.events().all();
    let (_, topics, _data) = events.last().unwrap();
    let action_id = BytesN::<32>::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    let signer = Address::try_from_val(&h.env, &topics.get(3).unwrap()).unwrap();
    assert_eq!(signer, h.signers[1]);
    assert_eq!(action_id, aid(&h.env, 7));
}

#[test]
fn reset_emits_a_reset_event_with_the_action_id_and_description_hash() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    h.client.reset(&aid(&h.env, 42));

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    assert_eq!(
        BytesN::<32>::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap(),
        aid(&h.env, 42)
    );
    assert_eq!(
        crate::ActionFinishedEvent::try_from_val(&h.env, &data).unwrap(),
        crate::ActionFinishedEvent {
            description_hash: BytesN::from_array(&h.env, &[0; 32])
        }
    );
}

// ─── require_auth is load-bearing where it exists, absent where it doesn't ─

/// Confirms initialize()'s lack of require_auth() is an intentional
/// design choice, not an untested oversight: it succeeds even with NO
/// auths mocked at all, unlike every other contract's initialize()
/// here (which all require the incoming admin's own signature). See
/// the doc comment on initialize() for why this is safe — approve()'s
/// own require_auth() is what actually gates anything.
#[test]
fn initialize_does_not_require_any_signers_authorization() {
    let env = Env::default(); // deliberately no mock_all_auths()
    let signers = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);

    client.initialize(
        &vec![
            &env,
            signers[0].clone(),
            signers[1].clone(),
            signers[2].clone(),
        ],
        &2,
        &0,
    );
    assert_eq!(client.get_signer_count(), 3);
}

#[test]
fn approve_without_any_authorization_fails() {
    // Unlike initialize(), approve() DOES require the signer's own
    // signature — this confirms that check is load-bearing, not a
    // no-op, by never arming mock_all_auths() in the first place.
    let env = Env::default();
    let signers = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(
        &vec![
            &env,
            signers[0].clone(),
            signers[1].clone(),
            signers[2].clone(),
        ],
        &2,
        &0,
    );

    // No auth mocked: the host rejects the signer's require_auth().
    assert!(matches!(
        client.try_approve(&signers[0], &aid(&env, 42)),
        Err(Err(_))
    ));
}

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

// ─── registry ───────────────────────────────────────────────────────────────

#[test]
fn first_approval_registers_the_action() {
    let h = setup();
    assert!(h.client.get_action(&aid(&h.env, 1)).is_none());
    h.env.ledger().with_mut(|l| l.timestamp = 500);
    h.client.approve(&h.signers[0], &aid(&h.env, 1));

    let meta = h.client.get_action(&aid(&h.env, 1)).unwrap();
    assert_eq!(meta.proposer, h.signers[0]);
    assert_eq!(meta.created_at, 500);
    assert_eq!(meta.status, ActionStatus::Pending);
    assert_eq!(h.client.get_pending_count(), 1);
}

#[test]
fn register_action_stores_the_description_hash_and_approvals_carry_it() {
    let h = setup();
    let desc = aid(&h.env, 0xAB);
    h.client
        .register_action(&h.signers[2], &aid(&h.env, 1), &desc);
    h.client.approve(&h.signers[0], &aid(&h.env, 1));

    let meta = h.client.get_action(&aid(&h.env, 1)).unwrap();
    assert_eq!(meta.proposer, h.signers[2]);
    assert_eq!(meta.description_hash, desc);
    let events = h.env.events().all();
    let (_, _, data) = events.last().unwrap();
    let approved = crate::ApprovedEvent::try_from_val(&h.env, &data).unwrap();
    assert_eq!(approved.description_hash, desc);
}

#[test]
fn register_action_rejects_a_pending_id() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    assert_eq!(
        h.client
            .try_register_action(&h.signers[1], &aid(&h.env, 1), &aid(&h.env, 2)),
        Err(Ok(Error::ActionAlreadyRegistered))
    );
}

#[test]
fn register_action_rejects_a_non_signer() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    assert_eq!(
        h.client
            .try_register_action(&stranger, &aid(&h.env, 1), &aid(&h.env, 2)),
        Err(Ok(Error::NotASigner))
    );
}

#[test]
fn reset_removes_the_action_from_the_pending_index() {
    let h = setup();
    for n in 1..=3u8 {
        h.client.approve(&h.signers[0], &aid(&h.env, n));
    }
    h.client.approve(&h.signers[1], &aid(&h.env, 1));
    h.client.reset(&aid(&h.env, 1));

    assert_eq!(h.client.get_pending_count(), 2);
    let page = h.client.get_pending_actions(&0, &MAX_PAGE_LIMIT);
    assert!(!page.contains(aid(&h.env, 1)));
    assert!(page.contains(aid(&h.env, 2)) && page.contains(aid(&h.env, 3)));
    assert_eq!(
        h.client.get_action(&aid(&h.env, 1)).unwrap().status,
        ActionStatus::Reset
    );

    // A reset id can be reused, starting over as a fresh pending action.
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    assert_eq!(
        h.client.get_action(&aid(&h.env, 1)).unwrap().status,
        ActionStatus::Pending
    );
}

#[test]
fn pending_actions_paginate_across_boundaries() {
    let h = setup();
    for n in 1..=5u8 {
        let signer = &h.signers[(n % 3) as usize];
        h.client.approve(signer, &aid(&h.env, n));
    }
    assert_eq!(h.client.get_pending_actions(&0, &2).len(), 2);
    assert_eq!(h.client.get_pending_actions(&2, &2).len(), 2);
    assert_eq!(h.client.get_pending_actions(&4, &2).len(), 1);
    assert_eq!(h.client.get_pending_actions(&5, &2).len(), 0);
    assert_eq!(h.client.get_pending_actions(&50, &2).len(), 0);

    let mut all = Vec::new(&h.env);
    for c in [0u32, 2, 4] {
        all.append(&h.client.get_pending_actions(&c, &2));
    }
    for n in 1..=5u8 {
        assert!(all.contains(aid(&h.env, n)));
    }
}

#[test]
fn pending_actions_rejects_an_oversized_limit() {
    let h = setup();
    assert_eq!(
        h.client.try_get_pending_actions(&0, &(MAX_PAGE_LIMIT + 1)),
        Err(Ok(Error::InvalidPageLimit))
    );
}

#[test]
fn a_signer_cannot_register_more_than_the_per_signer_cap() {
    let h = setup();
    for n in 0..MAX_PENDING_PER_SIGNER {
        h.client.approve(&h.signers[0], &aid(&h.env, n as u8));
    }
    assert_eq!(
        h.client
            .try_approve(&h.signers[0], &aid(&h.env, MAX_PENDING_PER_SIGNER as u8)),
        Err(Ok(Error::TooManyPendingActions))
    );
}

#[test]
fn stale_actions_can_be_expired_out_of_the_index() {
    let h = setup_with_ttl(100);
    h.env.ledger().with_mut(|l| l.timestamp = 1_000);
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.env.ledger().with_mut(|l| l.timestamp = 1_101);

    h.client.expire(&aid(&h.env, 1));
    assert_eq!(h.client.get_pending_count(), 0);
    assert_eq!(
        h.client.get_action(&aid(&h.env, 1)).unwrap().status,
        ActionStatus::Expired
    );
}

#[test]
fn expire_rejects_an_action_with_a_fresh_approval() {
    let h = setup_with_ttl(100);
    h.env.ledger().with_mut(|l| l.timestamp = 1_000);
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.env.ledger().with_mut(|l| l.timestamp = 1_101);
    h.client.approve(&h.signers[1], &aid(&h.env, 1));
    assert_eq!(
        h.client.try_expire(&aid(&h.env, 1)),
        Err(Ok(Error::NotExpired))
    );
}

#[test]
fn expire_is_unavailable_without_an_approval_ttl() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.client.revoke(&h.signers[0], &aid(&h.env, 1));
    assert_eq!(
        h.client.try_expire(&aid(&h.env, 1)),
        Err(Ok(Error::NotExpired))
    );
}

// ─── executor ───────────────────────────────────────────────────────────────

/// Minimal target: `admin`-gated setter, plus a function that always fails.
#[contract]
pub struct Target;

#[contractimpl]
impl Target {
    pub fn init(env: Env, admin: Address) {
        env.storage()
            .instance()
            .set(&symbol_short!("admin"), &admin);
    }

    pub fn set(env: Env, value: u32) -> u32 {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .unwrap();
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("value"), &value);
        value
    }

    pub fn get(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("value"))
            .unwrap_or(0)
    }

    pub fn fail(_env: Env) {
        panic!("target failed");
    }
}

fn setup_target(h: &Harness) -> TargetClient<'static> {
    let id = h.env.register_contract(None, Target);
    let target = TargetClient::new(&h.env, &id);
    target.init(&h.client.address);
    target
}

fn set_args(env: &Env, value: u32) -> Vec<Val> {
    vec![env, value.into_val(env)]
}

#[test]
fn execute_invokes_the_target_as_the_multisig() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &set_args(&h.env, 7),
    );
    assert_eq!(h.client.get_action(&id).unwrap().description_hash, id);
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);

    // No mocked auths from here on: the target's admin.require_auth()
    // must be satisfied purely because the multisig is the direct caller.
    h.env.set_auths(&[]);
    let ret = h.client.execute(&id);
    assert_eq!(u32::try_from_val(&h.env, &ret).unwrap(), 7);
    assert_eq!(target.get(), 7);
    assert_eq!(
        h.client.get_action(&id).unwrap().status,
        ActionStatus::Executed
    );
    assert_eq!(h.client.get_pending_count(), 0);
}

#[test]
fn identical_proposals_get_distinct_ids() {
    let h = setup();
    let target = setup_target(&h);
    let f = Symbol::new(&h.env, "set");
    let a = h
        .client
        .propose(&h.signers[0], &target.address, &f, &set_args(&h.env, 1));
    let b = h
        .client
        .propose(&h.signers[0], &target.address, &f, &set_args(&h.env, 1));
    assert_ne!(a, b);
    assert_eq!(h.client.get_proposal(&a).unwrap().nonce, 0);
}

#[test]
fn executed_proposals_cannot_be_replayed() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &set_args(&h.env, 7),
    );
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);
    h.client.execute(&id);
    assert!(matches!(
        h.client.try_execute(&id),
        Err(Ok(Error::ActionNotPending))
    ));
}

#[test]
fn executed_proposals_cannot_be_re_approved() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &set_args(&h.env, 7),
    );
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);
    h.client.execute(&id);
    assert_eq!(
        h.client.try_approve(&h.signers[0], &id),
        Err(Ok(Error::ActionNotPending))
    );
}

#[test]
fn execute_requires_threshold() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &set_args(&h.env, 7),
    );
    h.client.approve(&h.signers[0], &id);
    assert!(matches!(
        h.client.try_execute(&id),
        Err(Ok(Error::NotYetApproved))
    ));
}

#[test]
fn a_failing_target_reverts_execute_entirely() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "fail"),
        &Vec::new(&h.env),
    );
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);

    assert!(h.client.try_execute(&id).is_err());
    // Nothing stuck: still pending, still approved, still executable.
    assert_eq!(
        h.client.get_action(&id).unwrap().status,
        ActionStatus::Pending
    );
    assert!(h.client.is_approved(&id));
}

#[test]
fn propose_rejects_oversized_argument_lists() {
    let h = setup();
    let target = setup_target(&h);
    let mut args = Vec::new(&h.env);
    for i in 0..=crate::MAX_PROPOSAL_ARGS {
        args.push_back(i.into_val(&h.env));
    }
    assert_eq!(
        h.client.try_propose(
            &h.signers[0],
            &target.address,
            &Symbol::new(&h.env, "set"),
            &args
        ),
        Err(Ok(Error::ProposalTooLarge))
    );
}

#[test]
fn proposals_cannot_be_reset_out_from_under_execute() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &set_args(&h.env, 7),
    );
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);
    assert_eq!(h.client.try_reset(&id), Err(Ok(Error::ActionNotPending)));
}

// ─── TTL policy (issue #98) and archival restore (issue #99) ────────────────

fn advance_ledgers(env: &Env, ledgers: u32) {
    use soroban_sdk::testutils::Ledger as _;
    env.ledger().with_mut(|l| l.sequence_number += ledgers);
}

/// Whether `key` (a persistent entry, or the instance when `None`) of
/// `contract` is still live. Accessing an archived entry through a
/// client aborts the test rather than returning an error, so archival is
/// checked straight against the ledger storage instead.
fn is_live(env: &Env, contract: &Address, key: Option<crate::types::DataKey>) -> bool {
    use soroban_sdk::xdr::{LedgerKey, ScAddress, ScVal};
    use soroban_sdk::IntoVal;
    let key = match key {
        Some(key) => {
            let val: soroban_sdk::Val = key.into_val(env);
            ScVal::try_from_val(env, &val).unwrap()
        }
        None => ScVal::LedgerKeyContractInstance,
    };
    let contract = ScAddress::from(contract);
    let seq = env.ledger().sequence();
    env.host()
        .with_mut_storage(|storage| {
            for (ledger_key, entry) in storage.map.clone() {
                if let LedgerKey::ContractData(data) = ledger_key.as_ref() {
                    if data.contract == contract && data.key == key {
                        return Ok(
                            matches!(entry, Some((_, Some(live_until))) if live_until >= seq),
                        );
                    }
                }
            }
            Ok(false)
        })
        .unwrap()
}

/// Simulates a `RestoreFootprint` operation over every archived
/// persistent entry: like the real operation, it brings the entry back
/// with its stored value untouched and a fresh
/// `min_persistent_entry_ttl` lifetime.
fn restore_archived(env: &Env) {
    use soroban_sdk::testutils::Ledger as _;
    use soroban_sdk::xdr::{ContractDataDurability, LedgerKey};
    let seq = env.ledger().sequence();
    let live_until = seq + env.ledger().get().min_persistent_entry_ttl - 1;
    let budget = env.host().budget_cloned();
    env.host()
        .with_mut_storage(|storage| {
            for (key, entry) in storage.map.clone() {
                let Some((entry, Some(old_live_until))) = entry else {
                    continue;
                };
                if old_live_until >= seq {
                    continue;
                }
                if let LedgerKey::ContractData(data) = key.as_ref() {
                    if data.durability == ContractDataDurability::Temporary {
                        continue;
                    }
                }
                storage.put(&key, &entry, Some(live_until), &budget)?;
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn ttl_policy_keeps_approvals_live_and_bump_extends_them() {
    use crate::{ttl, types::DataKey};
    let h = setup();
    let id = aid(&h.env, 7);
    h.client.approve(&h.signers[0], &id);
    let addr = h.client.address.clone();
    for _ in 0..5 {
        advance_ledgers(&h.env, 20 * ttl::DAY_IN_LEDGERS);
        h.client
            .bump(&vec![&h.env, DataKey::Approvals(0, id.clone())]);
    }
    assert!(is_live(&h.env, &addr, None));
    assert!(is_live(
        &h.env,
        &addr,
        Some(DataKey::Approvals(0, id.clone()))
    ));
    assert!(h.client.has_approved(&id, &h.signers[0]));
}

#[test]
fn archived_approval_restores_and_counts_identically() {
    use crate::{ttl, types::DataKey};
    let h = setup();
    let id = aid(&h.env, 7);
    h.client.approve(&h.signers[0], &id);
    let addr = h.client.address.clone();
    for _ in 0..5 {
        advance_ledgers(&h.env, 20 * ttl::DAY_IN_LEDGERS);
        h.client.get_threshold();
    }
    assert!(!is_live(
        &h.env,
        &addr,
        Some(DataKey::Approvals(0, id.clone()))
    ));

    restore_archived(&h.env);

    assert_eq!(h.client.get_approval_count(&id), 1);
    h.client.approve(&h.signers[1], &id);
    assert!(h.client.is_approved(&id));
}

#[test]
fn archived_instance_restores_signers_and_threshold() {
    use crate::ttl;
    let h = setup();
    advance_ledgers(&h.env, ttl::INSTANCE_BUMP_AMOUNT + 1);
    assert!(!is_live(&h.env, &h.client.address, None));

    restore_archived(&h.env);

    assert_eq!(h.client.get_signer_count(), 3);
    assert_eq!(h.client.get_threshold(), 2);
}

// ── Weighted signers ─────────────────────────────────────────────────────────

use crate::types::SignerWeight;

fn setup_weighted<'a>(
    weights: &[u32],
    threshold: u32,
) -> (Env, MultisigClient<'a>, std::vec::Vec<Address>) {
    let env = Env::default();
    env.mock_all_auths();
    let addrs: std::vec::Vec<Address> = weights.iter().map(|_| Address::generate(&env)).collect();
    let mut signers = Vec::new(&env);
    for (a, w) in addrs.iter().zip(weights) {
        signers.push_back((a.clone(), *w));
    }
    let total: u32 = weights.iter().sum();
    let client = MultisigClient::new(&env, &env.register_contract(None, Multisig));
    client.initialize_weighted(&signers, &threshold, &0, &total, &100);
    (env, client, addrs)
}

/// `setup_weighted`'s initialize, returning its typed error.
fn try_setup_weighted(weights: &[u32], threshold: u32) -> Result<(), Error> {
    let env = Env::default();
    env.mock_all_auths();
    let mut signers = Vec::new(&env);
    for w in weights {
        signers.push_back((Address::generate(&env), *w));
    }
    let total: u32 = weights.iter().sum();
    let client = MultisigClient::new(&env, &env.register_contract(None, Multisig));
    match client.try_initialize_weighted(&signers, &threshold, &0, &total, &100) {
        Ok(_) => Ok(()),
        Err(Ok(e)) => Err(e),
        Err(Err(e)) => panic!("initialize_weighted failed in the host: {e:?}"),
    }
}

fn has_event(env: &Env, name: &str) -> bool {
    let name = Symbol::new(env, name);
    env.events().all().iter().any(|(_, topics, _)| {
        topics
            .get(0)
            .and_then(|t| Symbol::try_from_val(env, &t).ok())
            .map(|s| s == name)
            .unwrap_or(false)
    })
}

#[test]
fn legacy_initialize_gives_equal_weights_and_higher_rotation_threshold() {
    let h = setup();
    for signer in &h.signers {
        assert_eq!(h.client.get_signer_weight(signer), 1);
    }
    assert_eq!(h.client.get_total_weight(), 3);
    assert_eq!(h.client.get_rotation_threshold(), 3);
    assert_eq!(h.client.get_rotation_delay(), crate::DEFAULT_ROTATION_DELAY);
    assert_eq!(h.client.get_signer_epoch(), 0);
}

#[test]
fn weighted_threshold_matrix() {
    // Two core members (weight 2) and three community members (weight 1),
    // threshold 4: any 2 core, or 1 core + 2 community — never 3 community.
    let (env, client, s) = setup_weighted(&[2, 2, 1, 1, 1], 4);
    let cases: [(&[usize], bool); 5] = [
        (&[0, 1], true),
        (&[0, 2, 3], true),
        (&[2, 3, 4], false),
        (&[0, 2], false),
        (&[1, 2, 3, 4], true),
    ];
    for (n, (voters, approved)) in cases.iter().enumerate() {
        let id = aid(&env, n as u8 + 1);
        let mut weight = 0;
        for &v in voters.iter() {
            client.approve(&s[v], &id);
            weight += client.get_signer_weight(&s[v]);
        }
        assert_eq!(client.get_approval_weight(&id), weight);
        assert_eq!(client.get_approval_count(&id), voters.len() as u32);
        assert_eq!(client.is_approved(&id), *approved);
    }
}

#[test]
fn a_signer_heavy_enough_to_act_alone_emits_a_warning() {
    let (env, client, s) = setup_weighted(&[5, 1, 1], 5);
    assert!(has_event(&env, "single_key_quorum"));
    let id = aid(&env, 1);
    client.approve(&s[0], &id);
    assert!(client.is_approved(&id));
}

#[test]
fn weighted_initialize_rejects_zero_weight() {
    assert_eq!(try_setup_weighted(&[1, 0], 1), Err(Error::InvalidWeight));
}

#[test]
fn weighted_initialize_rejects_total_weight_overflow() {
    let env = Env::default();
    let client = MultisigClient::new(&env, &env.register_contract(None, Multisig));
    let signers = vec![
        &env,
        (Address::generate(&env), u32::MAX),
        (Address::generate(&env), 1u32),
    ];
    assert_eq!(
        client.try_initialize_weighted(&signers, &1, &0, &2, &0),
        Err(Ok(Error::WeightOverflow))
    );
}

#[test]
fn weighted_initialize_rejects_threshold_above_total_weight() {
    assert_eq!(try_setup_weighted(&[2, 1], 4), Err(Error::InvalidThreshold));
}

#[test]
fn weighted_initialize_rejects_rotation_threshold_not_above_threshold() {
    let env = Env::default();
    let client = MultisigClient::new(&env, &env.register_contract(None, Multisig));
    let signers = vec![
        &env,
        (Address::generate(&env), 1u32),
        (Address::generate(&env), 1u32),
        (Address::generate(&env), 1u32),
    ];
    assert_eq!(
        client.try_initialize_weighted(&signers, &2, &0, &2, &0),
        Err(Ok(Error::InvalidThreshold))
    );
}

#[test]
fn weighted_initialize_rejects_duplicates() {
    let env = Env::default();
    let client = MultisigClient::new(&env, &env.register_contract(None, Multisig));
    let a = Address::generate(&env);
    assert_eq!(
        client.try_initialize_weighted(&vec![&env, (a.clone(), 1u32), (a, 2u32)], &1, &0, &3, &0),
        Err(Ok(Error::DuplicateSigner))
    );
}

// ── Signer rotation ──────────────────────────────────────────────────────────

fn none_added(env: &Env) -> Vec<SignerWeight> {
    Vec::new(env)
}

fn add(env: &Env, address: &Address, weight: u32) -> Vec<SignerWeight> {
    vec![
        env,
        SignerWeight {
            address: address.clone(),
            weight,
        },
    ]
}

fn approve_all(h: &Harness, id: &BytesN<32>) {
    for s in &h.signers {
        h.client.approve(s, id);
    }
}

fn pass_delay(env: &Env) {
    let now = env.ledger().timestamp();
    env.ledger()
        .with_mut(|l| l.timestamp = now + crate::DEFAULT_ROTATION_DELAY);
}

#[test]
fn rotation_replaces_a_signer_after_the_delay_and_invalidates_votes() {
    let h = setup();
    let newcomer = Address::generate(&h.env);

    // A consumer action sitting at threshold before the rotation.
    let consumer = aid(&h.env, 9);
    h.client.approve(&h.signers[0], &consumer);
    h.client.approve(&h.signers[1], &consumer);
    assert!(h.client.is_approved(&consumer));

    let id = h.client.propose_signer_change(
        &h.signers[0],
        &add(&h.env, &newcomer, 1),
        &vec![&h.env, h.signers[2].clone()],
        &2,
        &3,
    );
    assert!(has_event(&h.env, "signer_change_proposed"));
    approve_all(&h, &id);
    let ready_at = h.client.queue_signer_change(&id);
    assert_eq!(
        ready_at,
        h.env.ledger().timestamp() + crate::DEFAULT_ROTATION_DELAY
    );

    pass_delay(&h.env);
    h.client.execute_signer_change(&id);
    assert!(has_event(&h.env, "signers_rotated"));

    assert_eq!(h.client.get_signer_epoch(), 1);
    assert!(h.client.is_signer(&newcomer));
    assert!(!h.client.is_signer(&h.signers[2]));
    assert_eq!(h.client.get_signer_count(), 3);
    // Votes from the previous epoch no longer count.
    assert!(!h.client.is_approved(&consumer));
    assert_eq!(h.client.get_approval_weight(&consumer), 0);
    assert_eq!(
        h.client.get_action(&id).unwrap().status,
        ActionStatus::Executed
    );
}

#[test]
fn rotation_can_change_a_signers_weight() {
    let h = setup();
    let id = h.client.propose_signer_change(
        &h.signers[0],
        &add(&h.env, &h.signers[0], 3),
        &vec![&h.env, h.signers[0].clone()],
        &3,
        &5,
    );
    approve_all(&h, &id);
    h.client.queue_signer_change(&id);
    pass_delay(&h.env);
    h.client.execute_signer_change(&id);
    assert_eq!(h.client.get_signer_weight(&h.signers[0]), 3);
    assert_eq!(h.client.get_total_weight(), 5);
    assert_eq!(h.client.get_threshold(), 3);
    assert_eq!(h.client.get_rotation_threshold(), 5);
}

#[test]
fn rotation_needs_the_rotation_threshold_not_the_normal_one() {
    let h = setup();
    let id = h.client.propose_signer_change(
        &h.signers[0],
        &none_added(&h.env),
        &Vec::new(&h.env),
        &1,
        &3,
    );
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);
    assert!(h.client.is_approved(&id)); // normal threshold met...
    assert_eq!(
        h.client.try_queue_signer_change(&id), // ...rotation threshold not.
        Err(Ok(Error::NotYetApproved))
    );
}

#[test]
fn rotation_cannot_execute_one_second_before_the_delay_ends() {
    let h = setup();
    let id = h.client.propose_signer_change(
        &h.signers[0],
        &none_added(&h.env),
        &Vec::new(&h.env),
        &1,
        &3,
    );
    approve_all(&h, &id);
    let ready_at = h.client.queue_signer_change(&id);
    h.env.ledger().with_mut(|l| l.timestamp = ready_at - 1);
    assert_eq!(
        h.client.try_execute_signer_change(&id),
        Err(Ok(Error::RotationDelayActive))
    );
}

#[test]
fn rotation_executes_exactly_at_the_delay_boundary() {
    let h = setup();
    let id = h.client.propose_signer_change(
        &h.signers[0],
        &none_added(&h.env),
        &Vec::new(&h.env),
        &1,
        &3,
    );
    approve_all(&h, &id);
    let ready_at = h.client.queue_signer_change(&id);
    h.env.ledger().with_mut(|l| l.timestamp = ready_at);
    h.client.execute_signer_change(&id);
    assert_eq!(h.client.get_threshold(), 1);
}

#[test]
fn rotation_cannot_execute_without_being_queued() {
    let h = setup();
    let id = h.client.propose_signer_change(
        &h.signers[0],
        &none_added(&h.env),
        &Vec::new(&h.env),
        &1,
        &3,
    );
    approve_all(&h, &id);
    assert_eq!(
        h.client.try_execute_signer_change(&id),
        Err(Ok(Error::SignerChangeNotQueued))
    );
}

#[test]
fn rotation_rechecks_approval_weight_at_execution() {
    let h = setup();
    let id = h.client.propose_signer_change(
        &h.signers[0],
        &none_added(&h.env),
        &Vec::new(&h.env),
        &1,
        &3,
    );
    approve_all(&h, &id);
    h.client.queue_signer_change(&id);
    h.client.revoke(&h.signers[1], &id);
    pass_delay(&h.env);
    assert_eq!(
        h.client.try_execute_signer_change(&id),
        Err(Ok(Error::NotYetApproved))
    );
}

#[test]
fn any_single_signer_can_veto_during_the_delay() {
    let h = setup();
    let id = h.client.propose_signer_change(
        &h.signers[0],
        &none_added(&h.env),
        &Vec::new(&h.env),
        &1,
        &3,
    );
    approve_all(&h, &id);
    h.client.queue_signer_change(&id);
    h.client.veto_signer_change(&h.signers[2], &id);
    assert!(has_event(&h.env, "signer_change_vetoed"));
    pass_delay(&h.env);
    assert_eq!(
        h.client.try_execute_signer_change(&id),
        Err(Ok(Error::ActionNotPending))
    );
}

#[test]
fn non_signers_cannot_veto() {
    let h = setup();
    let id = h.client.propose_signer_change(
        &h.signers[0],
        &none_added(&h.env),
        &Vec::new(&h.env),
        &1,
        &3,
    );
    assert_eq!(
        h.client
            .try_veto_signer_change(&Address::generate(&h.env), &id),
        Err(Ok(Error::NotASigner))
    );
}

#[test]
fn a_rotation_from_a_previous_epoch_cannot_execute() {
    let h = setup();
    let first = h.client.propose_signer_change(
        &h.signers[0],
        &none_added(&h.env),
        &Vec::new(&h.env),
        &1,
        &3,
    );
    let second = h.client.propose_signer_change(
        &h.signers[1],
        &none_added(&h.env),
        &Vec::new(&h.env),
        &2,
        &3,
    );
    approve_all(&h, &first);
    approve_all(&h, &second);
    h.client.queue_signer_change(&first);
    h.client.queue_signer_change(&second);
    pass_delay(&h.env);
    h.client.execute_signer_change(&first);
    assert_eq!(
        h.client.try_execute_signer_change(&second),
        Err(Ok(Error::StaleSignerChange))
    );
}

#[test]
fn rotation_rejects_removing_a_non_signer() {
    let h = setup();
    assert_eq!(
        h.client.try_propose_signer_change(
            &h.signers[0],
            &none_added(&h.env),
            &vec![&h.env, Address::generate(&h.env)],
            &2,
            &3
        ),
        Err(Ok(Error::NotASigner))
    );
}

#[test]
fn rotation_rejects_adding_an_existing_signer() {
    let h = setup();
    assert_eq!(
        h.client.try_propose_signer_change(
            &h.signers[0],
            &add(&h.env, &h.signers[1], 1),
            &Vec::new(&h.env),
            &2,
            &4
        ),
        Err(Ok(Error::DuplicateSigner))
    );
}

#[test]
fn rotation_rejects_more_than_one_addition() {
    let h = setup();
    let mut two = add(&h.env, &Address::generate(&h.env), 1);
    two.push_back(SignerWeight {
        address: Address::generate(&h.env),
        weight: 1,
    });
    assert_eq!(
        h.client
            .try_propose_signer_change(&h.signers[0], &two, &Vec::new(&h.env), &2, &3),
        Err(Ok(Error::InvalidSignerChange))
    );
}

#[test]
fn rotation_rejects_a_zero_weight_signer() {
    let h = setup();
    assert_eq!(
        h.client.try_propose_signer_change(
            &h.signers[0],
            &add(&h.env, &Address::generate(&h.env), 0),
            &Vec::new(&h.env),
            &2,
            &3
        ),
        Err(Ok(Error::InvalidWeight))
    );
}

#[test]
fn rotation_rejects_a_threshold_left_unreachable_by_a_removal() {
    let h = setup();
    assert_eq!(
        h.client.try_propose_signer_change(
            &h.signers[0],
            &none_added(&h.env),
            &vec![&h.env, h.signers[2].clone()],
            &3,
            &3
        ),
        Err(Ok(Error::InvalidThreshold))
    );
}

#[test]
fn rotation_rejects_a_zero_threshold() {
    let h = setup();
    assert_eq!(
        h.client.try_propose_signer_change(
            &h.signers[0],
            &none_added(&h.env),
            &Vec::new(&h.env),
            &0,
            &3
        ),
        Err(Ok(Error::InvalidThreshold))
    );
}

#[test]
fn rotation_revalidates_the_rotation_threshold() {
    let h = setup();
    // Rotation threshold equal to the threshold without being unanimous.
    assert_eq!(
        h.client.try_propose_signer_change(
            &h.signers[0],
            &none_added(&h.env),
            &Vec::new(&h.env),
            &2,
            &2
        ),
        Err(Ok(Error::InvalidThreshold))
    );
}

#[test]
fn signer_changes_cannot_be_reset_through_the_generic_path() {
    let h = setup();
    let id = h.client.propose_signer_change(
        &h.signers[0],
        &none_added(&h.env),
        &Vec::new(&h.env),
        &1,
        &3,
    );
    approve_all(&h, &id);
    assert_eq!(h.client.try_reset(&id), Err(Ok(Error::ActionNotPending)));
}

/// Benchmark (run with `--nocapture`): CPU cost of `is_approved` with 16
/// signers, all approved. Read-entry count is 1 + N per call before the
/// single-entry bitmap storage, and 1 (+ instance) after.
#[test]
fn bench_is_approved_16_signers() {
    let env = Env::default();
    env.mock_all_auths();
    let mut signers = Vec::new(&env);
    for _ in 0..16 {
        signers.push_back(Address::generate(&env));
    }
    let client = MultisigClient::new(&env, &env.register_contract(None, Multisig));
    client.initialize(&signers, &16, &0);
    let id = aid(&env, 1);
    for s in signers.iter() {
        client.approve(&s, &id);
    }
    env.budget().reset_default();
    assert!(client.is_approved(&id));
    std::println!(
        "BENCH is_approved cpu={}",
        env.budget().cpu_instruction_cost()
    );
}

// ─── veto, deadlines, prune, bitmap storage ─────────────────────────────────

fn set_time(env: &Env, ts: u64) {
    env.ledger().with_mut(|l| l.timestamp = ts);
}

fn with_ttl<'a>(ttl: u64) -> Harness<'a> {
    let h = setup();
    let env = h.env.clone();
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(&Vec::from_array(&env, h.signers.clone()), &2, &ttl);
    Harness {
        env,
        client,
        signers: h.signers,
    }
}

#[test]
fn veto_blocks_approval_permanently() {
    let h = setup();
    let vetoer = Address::generate(&h.env);
    h.client.set_vetoer(&vetoer);
    let id = aid(&h.env, 1);
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);
    assert!(h.client.is_approved(&id));
    h.client.veto(&vetoer, &id);
    assert!(!h.client.is_approved(&id));
    assert_eq!(
        h.client.get_action(&id).unwrap().status,
        ActionStatus::Vetoed
    );
    assert_eq!(
        h.client.try_approve(&h.signers[2], &id),
        Err(Ok(Error::ActionNotPending))
    );
    assert!(!h.client.is_approved(&id));
    // A second veto, or one on a finished action, is rejected.
    assert_eq!(
        h.client.try_veto(&vetoer, &id),
        Err(Ok(Error::ActionNotPending))
    );
}

#[test]
fn veto_rejects_non_vetoer_and_executed_actions() {
    let h = setup();
    let vetoer = Address::generate(&h.env);
    let id = aid(&h.env, 1);
    h.client.approve(&h.signers[0], &id);
    assert_eq!(h.client.try_veto(&vetoer, &id), Err(Ok(Error::NotVetoer))); // none configured
    h.client.set_vetoer(&vetoer);
    assert_eq!(
        h.client.try_set_vetoer(&vetoer),
        Err(Ok(Error::VetoerAlreadySet))
    ); // immutable
    assert_eq!(
        h.client.try_veto(&h.signers[0], &id),
        Err(Ok(Error::NotVetoer))
    );
    h.client.approve(&h.signers[1], &id);
    h.client.reset(&id);
    assert_eq!(
        h.client.try_veto(&vetoer, &id),
        Err(Ok(Error::ActionNotPending))
    );
}

#[test]
fn execute_by_deadline_boundary() {
    let h = setup();
    set_time(&h.env, 100);
    let id = aid(&h.env, 1);
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);
    assert_eq!(
        h.client.try_set_execute_by(&h.signers[1], &id, &200),
        Err(Ok(Error::NotProposer))
    ); // not proposer
    assert_eq!(
        h.client.try_set_execute_by(&h.signers[0], &id, &100),
        Err(Ok(Error::InvalidDeadline))
    ); // not future
    h.client.set_execute_by(&h.signers[0], &id, &200);
    assert_eq!(
        h.client.try_set_execute_by(&h.signers[0], &id, &300),
        Err(Ok(Error::DeadlineAlreadySet))
    ); // once
    set_time(&h.env, 200);
    assert!(h.client.is_approved(&id));
    set_time(&h.env, 201);
    assert!(!h.client.is_approved(&id));
    h.client.expire(&id);
    assert_eq!(
        h.client.get_action(&id).unwrap().status,
        ActionStatus::Expired
    );
    assert!(!h.client.is_approved(&id));
}

#[test]
fn prune_never_changes_is_approved() {
    let h = with_ttl(100);
    set_time(&h.env, 1_000);
    let (live, stale, done) = (aid(&h.env, 1), aid(&h.env, 2), aid(&h.env, 3));
    h.client.approve(&h.signers[0], &stale);
    h.client.approve(&h.signers[0], &done);
    h.client.approve(&h.signers[1], &done);
    h.client.reset(&done);
    set_time(&h.env, 1_500);
    h.client.approve(&h.signers[0], &live);
    h.client.approve(&h.signers[1], &live);
    let ids = vec![
        &h.env,
        live.clone(),
        stale.clone(),
        done.clone(),
        aid(&h.env, 9),
    ];
    let before: std::vec::Vec<bool> = ids.iter().map(|i| h.client.is_approved(&i)).collect();
    assert_eq!(h.client.prune(&ids), 1); // only `stale`; `done` already cleared
    let after: std::vec::Vec<bool> = ids.iter().map(|i| h.client.is_approved(&i)).collect();
    assert_eq!(before, after);
    assert_eq!(h.client.get_approval_count(&live), 2);
    assert_eq!(
        h.client.get_action(&done).unwrap().status,
        ActionStatus::Reset
    );
}

#[test]
fn prune_is_bounded() {
    let h = setup();
    let mut ids = Vec::new(&h.env);
    for i in 0..=crate::MAX_PRUNE as u8 {
        ids.push_back(aid(&h.env, i));
    }
    assert_eq!(h.client.try_prune(&ids), Err(Ok(Error::InvalidPageLimit)));
}

#[test]
fn temporary_approvals_lapse_with_the_ttl() {
    let h = with_ttl(100);
    set_time(&h.env, 1_000);
    let id = aid(&h.env, 1);
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);
    assert!(h.client.is_approved(&id));
    // Past the ttl in both timestamp and ledgers: gone either way.
    set_time(&h.env, 1_101);
    advance_ledgers(&h.env, 100);
    assert!(!h.client.is_approved(&id));
    assert_eq!(h.client.get_approval_count(&id), 0);
    // Re-approving after expiry works.
    h.client.approve(&h.signers[0], &id);
    assert!(h.client.has_approved(&id, &h.signers[0]));
}

#[test]
fn more_than_32_signers_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let mut s = Vec::new(&env);
    for _ in 0..33 {
        s.push_back(Address::generate(&env));
    }
    let client = MultisigClient::new(&env, &env.register_contract(None, Multisig));
    assert_eq!(
        client.try_initialize(&s, &2, &0),
        Err(Ok(Error::TooManySigners))
    );
}
