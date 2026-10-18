#![cfg(test)]

extern crate std;

use crate::{Vault, VaultClient};
use multisig::{Multisig, MultisigClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _},
    token, Address, BytesN, Env, TryFromVal,
};

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

struct Harness<'a> {
    env: Env,
    client: VaultClient<'a>,
    token: Address,
    admin: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin);
    let token_address = token_contract.address();

    let contract_id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &contract_id);
    client.initialize(&admin, &token_address);

    Harness {
        env,
        client,
        token: token_address,
        admin,
    }
}

fn mint(h: &Harness, to: &Address, amount: i128) {
    token::StellarAssetClient::new(&h.env, &h.token).mint(to, &amount);
}

fn balance(h: &Harness, of: &Address) -> i128 {
    token::Client::new(&h.env, &h.token).balance(of)
}

#[test]
fn initialize_sets_admin_and_token() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);
    assert_eq!(h.client.get_token(), h.token);
    assert_eq!(h.client.get_total_escrowed(), 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn initialize_twice_panics() {
    let h = setup();
    h.client.initialize(&h.admin, &h.token);
}

#[test]
#[should_panic] // no auth was mocked at all — require_auth() has nothing to accept
fn initialize_without_any_authorization_panics() {
    // Deliberately skips mock_all_auths(): every other test in this file
    // uses it (mock_all_auths() arms for the env's whole lifetime once
    // called, so there's no way to "unmock" partway through a test to
    // check a LATER call specifically) — this test exists just to
    // confirm require_auth() is actually load-bearing on initialize(),
    // not a no-op, by never arming it in the first place. Mirrors
    // options_market's identically-named test.
    let env = Env::default();
    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &contract_id);
    client.initialize(&admin, &token_address);
}

// ─── deposit ─────────────────────────────────────────────────────────────────

#[test]
fn deposit_pulls_funds_and_credits_the_tags_ledger() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit(&depositor, &7, &400);

    assert_eq!(h.client.balance_of(&7), 400);
    assert_eq!(h.client.get_total_escrowed(), 400);
    assert_eq!(balance(&h, &depositor), 600);
    assert_eq!(balance(&h, &h.client.address), 400);
}

#[test]
fn deposit_accumulates_across_multiple_calls_for_the_same_tag() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit(&depositor, &7, &100);
    h.client.deposit(&depositor, &7, &250);

    assert_eq!(h.client.balance_of(&7), 350);
    assert_eq!(h.client.get_total_escrowed(), 350);
}

#[test]
fn deposit_keeps_different_tags_independent() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit(&depositor, &1, &100);
    h.client.deposit(&depositor, &2, &250);

    assert_eq!(h.client.balance_of(&1), 100);
    assert_eq!(h.client.balance_of(&2), 250);
    assert_eq!(h.client.get_total_escrowed(), 350);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidAmount
fn deposit_rejects_a_non_positive_amount() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &0);
}

// ─── withdraw ────────────────────────────────────────────────────────────────

#[test]
fn withdraw_pays_out_and_debits_the_tags_ledger() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    let payee = Address::generate(&h.env);
    h.client.withdraw(&7, &payee, &150);

    assert_eq!(h.client.balance_of(&7), 250);
    assert_eq!(h.client.get_total_escrowed(), 250);
    assert_eq!(balance(&h, &payee), 150);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn withdraw_rejects_more_than_the_tags_own_balance() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    // Tag 7 only has 400 earmarked — this is exactly the gap the vault
    // exists to close: a withdrawal can't draw on tag 8's (nonexistent)
    // balance just because the vault's raw token balance happens to be
    // nonzero from OTHER deposits.
    let payee = Address::generate(&h.env);
    h.client.withdraw(&7, &payee, &401);
}

#[test]
fn withdraw_cannot_drain_another_tags_deposit() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &100);
    h.client.deposit(&depositor, &2, &900);

    let payee = Address::generate(&h.env);
    h.client.withdraw(&1, &payee, &100);

    // Tag 1 is now fully withdrawn; tag 2's much larger balance must be
    // completely unaffected.
    assert_eq!(h.client.balance_of(&1), 0);
    assert_eq!(h.client.balance_of(&2), 900);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidAmount
fn withdraw_rejects_a_non_positive_amount() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);
    h.client.withdraw(&7, &depositor, &0);
}

// ─── transfer_admin ─────────────────────────────────────────────────────────

#[test]
fn transfer_admin_hands_off_control() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);

    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);
    // Two-step: nothing moves until the new admin accepts.
    assert_eq!(h.client.get_admin(), h.admin);
    assert_eq!(h.client.get_pending_admin(), Some(new_admin.clone()));
    h.client.accept_admin();
    assert_eq!(h.client.get_admin(), new_admin);
}

#[test]
fn transfer_admin_emits_an_admin_transferred_event_with_old_and_new_admin() {
    let h = setup();
    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);
    h.client.accept_admin();

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (old_admin, event_new_admin) = <(Address, Address)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(old_admin, h.admin);
    assert_eq!(event_new_admin, new_admin);
}

// ─── pause / unpause ────────────────────────────────────────────────────────

#[test]
fn pause_blocks_deposit_and_withdraw_unpause_restores_them() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    assert!(!h.client.is_paused());
    h.client.pause();
    assert!(h.client.is_paused());

    h.client.unpause();
    assert!(!h.client.is_paused());

    // Both sides work again post-unpause.
    h.client.deposit(&depositor, &7, &100);
    h.client.withdraw(&7, &depositor, &50);
    assert_eq!(h.client.balance_of(&7), 450);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // ContractPaused
fn deposit_is_rejected_while_paused() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.pause();
    h.client.deposit(&depositor, &7, &400);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // ContractPaused
fn withdraw_is_rejected_while_paused() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    h.client.pause();
    h.client.withdraw(&7, &depositor, &100);
}

// ─── events ──────────────────────────────────────────────────────────────────

#[test]
fn deposit_emits_a_deposited_event_with_the_amount_as_data() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit(&depositor, &7, &400);

    let events = h.env.events().all();
    let (contract_id, topics, data) = events.last().unwrap();
    assert_eq!(contract_id, h.client.address);
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 400);

    // from and tag live in topics, not data — an indexer filtering
    // "deposits from this wallet" or "deposits for this tag" reads
    // them from here.
    let topic_from = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let topic_tag = u64::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(topic_from, depositor);
    assert_eq!(topic_tag, 7);
}

#[test]
fn withdraw_emits_a_withdrawn_event_with_the_amount_as_data() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    h.client.withdraw(&7, &depositor, &150);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 150);

    // to and tag live in topics, not data.
    let topic_to = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let topic_tag = u64::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(topic_to, depositor);
    assert_eq!(topic_tag, 7);
}

// ─── sweep_untagged ─────────────────────────────────────────────────────────

#[test]
fn sweep_untagged_recovers_a_direct_transfer_that_bypassed_deposit() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    // Simulate a stray direct transfer straight to the vault's own
    // address, bypassing deposit() entirely — those funds aren't credited
    // to any tag.
    mint(&h, &h.client.address, 250);

    let rescuer = Address::generate(&h.env);
    let swept = h.client.sweep_untagged(&rescuer);

    assert_eq!(swept, 250);
    assert_eq!(balance(&h, &rescuer), 250);
    // Tag 7's own escrowed balance must be completely untouched.
    assert_eq!(h.client.balance_of(&7), 400);
    assert_eq!(h.client.get_total_escrowed(), 400);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // NoUntaggedFunds
fn sweep_untagged_rejects_when_the_balance_exactly_matches_the_ledger() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    // No stray transfer this time — the vault's actual balance (400)
    // exactly matches get_total_escrowed (400), so there's nothing to
    // sweep.
    let rescuer = Address::generate(&h.env);
    h.client.sweep_untagged(&rescuer);
}

#[test]
fn sweep_untagged_emits_a_swept_untagged_event() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);
    mint(&h, &h.client.address, 250);

    let rescuer = Address::generate(&h.env);
    h.client.sweep_untagged(&rescuer);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 250);

    // to lives in topics, not data.
    let topic_to = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    assert_eq!(topic_to, rescuer);
}

// ─── transfer_tag ───────────────────────────────────────────────────────────

#[test]
fn transfer_tag_moves_escrow_without_any_token_movement() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);

    let vault_balance_before = balance(&h, &h.client.address);
    h.client.transfer_tag(&1, &2, &150);

    assert_eq!(h.client.balance_of(&1), 250);
    assert_eq!(h.client.balance_of(&2), 150);
    // TotalEscrowed unchanged — nothing entered or left the vault.
    assert_eq!(h.client.get_total_escrowed(), 400);
    assert_eq!(balance(&h, &h.client.address), vault_balance_before);
}

#[test]
fn transfer_tag_accumulates_into_an_already_funded_destination() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);
    h.client.deposit(&depositor, &2, &100);

    h.client.transfer_tag(&1, &2, &400);

    assert_eq!(h.client.balance_of(&1), 0);
    assert_eq!(h.client.balance_of(&2), 500);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn transfer_tag_rejects_more_than_the_source_tags_balance() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &100);

    h.client.transfer_tag(&1, &2, &101);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidAmount
fn transfer_tag_rejects_a_non_positive_amount() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &100);

    h.client.transfer_tag(&1, &2, &0);
}

#[test]
fn transfer_tag_emits_a_tag_transferred_event() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);

    h.client.transfer_tag(&1, &2, &150);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 150);

    // from_tag and to_tag live in topics, not data.
    let topic_from_tag = u64::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let topic_to_tag = u64::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(topic_from_tag, 1);
    assert_eq!(topic_to_tag, 2);
}

// ─── cross-contract: pause_via_multisig / transfer_admin_via_multisig ─────

/// Deploys a real multisig contract in the SAME Env as the vault under
/// test, with a 2-of-3 threshold.
fn setup_multisig(h: &Harness) -> (Address, [Address; 3]) {
    let signers = [
        Address::generate(&h.env),
        Address::generate(&h.env),
        Address::generate(&h.env),
    ];
    let contract_id = h.env.register_contract(None, Multisig);
    let client = MultisigClient::new(&h.env, &contract_id);
    client.initialize(
        &soroban_sdk::vec![
            &h.env,
            signers[0].clone(),
            signers[1].clone(),
            signers[2].clone()
        ],
        &2,
        &0,
    );
    // Only the trusted multisig is accepted by `_via_multisig` paths.
    h.client.set_multisig(&contract_id);
    (contract_id, signers)
}

#[test]
fn pause_via_multisig_pauses_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 1);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    assert!(!h.client.is_paused());
    h.client.pause_via_multisig(&multisig_id, &action_id);
    assert!(h.client.is_paused());
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn pause_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    multisig_client.approve(&signers[0], &aid(&h.env, 1)); // only 1 of 3

    h.client.pause_via_multisig(&multisig_id, &aid(&h.env, 1));
}

#[test]
fn unpause_via_multisig_unpauses_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    h.client.pause();
    assert!(h.client.is_paused());

    let action_id = aid(&h.env, 3);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client.unpause_via_multisig(&multisig_id, &action_id);
    assert!(!h.client.is_paused());
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn unpause_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client.pause();
    h.client
        .unpause_via_multisig(&multisig_id, &aid(&h.env, 99));
}

#[test]
fn transfer_admin_via_multisig_hands_off_control_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 2);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let new_admin = Address::generate(&h.env);
    h.client
        .transfer_admin_via_multisig(&multisig_id, &action_id, &new_admin);
    h.client.accept_admin();
    assert_eq!(h.client.get_admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn transfer_admin_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);
    let new_admin = Address::generate(&h.env);

    h.client
        .transfer_admin_via_multisig(&multisig_id, &aid(&h.env, 99), &new_admin);
}

// ─── cross-contract: withdraw_via_multisig ─────────────────────────────────

#[test]
fn withdraw_via_multisig_pays_out_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 4);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let payee = Address::generate(&h.env);
    h.client
        .withdraw_via_multisig(&multisig_id, &action_id, &7, &payee, &150);

    assert_eq!(h.client.balance_of(&7), 250);
    assert_eq!(balance(&h, &payee), 150);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn withdraw_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);
    let (multisig_id, _signers) = setup_multisig(&h);

    let payee = Address::generate(&h.env);
    h.client
        .withdraw_via_multisig(&multisig_id, &aid(&h.env, 99), &7, &payee, &150);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn withdraw_via_multisig_still_enforces_the_tags_own_balance_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 5);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    // Approval authorizes WHO can call this, not draining more than
    // tag 7 actually has earmarked.
    let payee = Address::generate(&h.env);
    h.client
        .withdraw_via_multisig(&multisig_id, &action_id, &7, &payee, &401);
}

// ─── cross-contract: transfer_tag_via_multisig ─────────────────────────────

#[test]
fn transfer_tag_via_multisig_moves_escrow_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 6);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .transfer_tag_via_multisig(&multisig_id, &action_id, &1, &2, &150);

    assert_eq!(h.client.balance_of(&1), 250);
    assert_eq!(h.client.balance_of(&2), 150);
    assert_eq!(h.client.get_total_escrowed(), 400); // unaffected by the reassignment
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn transfer_tag_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client
        .transfer_tag_via_multisig(&multisig_id, &aid(&h.env, 99), &1, &2, &150);
}

// ─── cross-contract: sweep_untagged_via_multisig ───────────────────────────

#[test]
fn sweep_untagged_via_multisig_recovers_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);
    // A stray direct transfer that bypassed deposit() entirely.
    mint(&h, &h.client.address, 100);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 7);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let recovered_to = Address::generate(&h.env);
    let swept = h
        .client
        .sweep_untagged_via_multisig(&multisig_id, &action_id, &recovered_to);

    assert_eq!(swept, 100);
    assert_eq!(balance(&h, &recovered_to), 100);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn sweep_untagged_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    mint(&h, &h.client.address, 100);
    let (multisig_id, _signers) = setup_multisig(&h);

    let recovered_to = Address::generate(&h.env);
    h.client
        .sweep_untagged_via_multisig(&multisig_id, &aid(&h.env, 99), &recovered_to);
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
fn ttl_policy_keeps_bumped_tags_live() {
    use crate::{ttl, types::DataKey};
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &1_000);
    for _ in 0..5 {
        advance_ledgers(&h.env, 20 * ttl::DAY_IN_LEDGERS);
        h.client
            .bump(&soroban_sdk::vec![&h.env, DataKey::Escrow(7)]);
    }
    assert!(is_live(&h.env, &h.client.address, Some(DataKey::Escrow(7))));
    assert_eq!(h.client.balance_of(&7), 1_000);
}

#[test]
fn archived_tag_restores_and_withdraws_identically() {
    use crate::{ttl, types::DataKey};
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &1_000);
    for _ in 0..5 {
        advance_ledgers(&h.env, 20 * ttl::DAY_IN_LEDGERS);
        h.client.is_paused();
    }
    assert!(!is_live(
        &h.env,
        &h.client.address,
        Some(DataKey::Escrow(7))
    ));

    restore_archived(&h.env);

    assert_eq!(h.client.balance_of(&7), 1_000);
    let to = Address::generate(&h.env);
    h.client.withdraw(&7, &to, &400);
    assert_eq!(balance(&h, &to), 400);
    assert_eq!(h.client.balance_of(&7), 600);
}

#[test]
fn archived_instance_restores_admin_and_token() {
    use crate::ttl;
    let h = setup();
    advance_ledgers(&h.env, ttl::INSTANCE_BUMP_AMOUNT + 1);
    assert!(!is_live(&h.env, &h.client.address, None));

    restore_archived(&h.env);

    assert_eq!(h.client.get_admin(), h.admin);
    assert_eq!(h.client.get_token(), h.token);
}
