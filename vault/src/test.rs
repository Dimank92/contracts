
extern crate std;

use crate::{Tag, Vault, VaultClient};
use multisig::{Multisig, MultisigClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events as _},
    token, vec, Address, BytesN, Env, Symbol, TryFromVal,
};

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

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

/// The namespaced tag a legacy u64 id maps to.
fn lt(h: &Harness, id: u64) -> Tag {
    Tag {
        owner: h.admin.clone(),
        kind: symbol_short!("legacy"),
        id,
    }
}

fn tag(owner: &Address, kind: &str, id: u64, env: &Env) -> Tag {
    Tag {
        owner: owner.clone(),
        kind: Symbol::new(env, kind),
        id,
    }
}

fn new_token(h: &Harness) -> Address {
    let token_admin = Address::generate(&h.env);
    h.env
        .register_stellar_asset_contract_v2(token_admin)
        .address()
}

#[test]
fn initialize_sets_admin_and_token() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);
    assert_eq!(h.client.get_token(), h.token);
    assert_eq!(h.client.get_total_escrowed(&h.token), 0);
    assert!(h.client.is_token_allowed(&h.token));
    assert_eq!(h.client.legacy_tag(&7), lt(&h, 7));
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

    h.client.deposit_legacy(&depositor, &7, &400);

    assert_eq!(h.client.balance_of_legacy(&7), 400);
    assert_eq!(h.client.get_total_escrowed(&h.token), 400);
    assert_eq!(balance(&h, &depositor), 600);
    assert_eq!(balance(&h, &h.client.address), 400);
}

#[test]
fn deposit_accumulates_across_multiple_calls_for_the_same_tag() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit_legacy(&depositor, &7, &100);
    h.client.deposit_legacy(&depositor, &7, &250);

    assert_eq!(h.client.balance_of_legacy(&7), 350);
    assert_eq!(h.client.get_total_escrowed(&h.token), 350);
}

#[test]
fn deposit_keeps_different_tags_independent() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit_legacy(&depositor, &1, &100);
    h.client.deposit_legacy(&depositor, &2, &250);

    assert_eq!(h.client.balance_of_legacy(&1), 100);
    assert_eq!(h.client.balance_of_legacy(&2), 250);
    assert_eq!(h.client.get_total_escrowed(&h.token), 350);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidAmount
fn deposit_rejects_a_non_positive_amount() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &0);
}

// ─── withdraw ────────────────────────────────────────────────────────────────

#[test]
fn withdraw_pays_out_and_debits_the_tags_ledger() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);

    let payee = Address::generate(&h.env);
    h.client.withdraw_legacy(&7, &payee, &150);

    assert_eq!(h.client.balance_of_legacy(&7), 250);
    assert_eq!(h.client.get_total_escrowed(&h.token), 250);
    assert_eq!(balance(&h, &payee), 150);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn withdraw_rejects_more_than_the_tags_own_balance() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);

    // Tag 7 only has 400 earmarked — this is exactly the gap the vault
    // exists to close: a withdrawal can't draw on tag 8's (nonexistent)
    // balance just because the vault's raw token balance happens to be
    // nonzero from OTHER deposits.
    let payee = Address::generate(&h.env);
    h.client.withdraw_legacy(&7, &payee, &401);
}

#[test]
fn withdraw_cannot_drain_another_tags_deposit() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &100);
    h.client.deposit_legacy(&depositor, &2, &900);

    let payee = Address::generate(&h.env);
    h.client.withdraw_legacy(&1, &payee, &100);

    // Tag 1 is now fully withdrawn; tag 2's much larger balance must be
    // completely unaffected.
    assert_eq!(h.client.balance_of_legacy(&1), 0);
    assert_eq!(h.client.balance_of_legacy(&2), 900);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidAmount
fn withdraw_rejects_a_non_positive_amount() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);
    h.client.withdraw_legacy(&7, &depositor, &0);
}

// ─── transfer_admin ─────────────────────────────────────────────────────────

#[test]
fn transfer_admin_hands_off_control() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);

    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);
    assert_eq!(h.client.get_admin(), new_admin);
}

#[test]
fn transfer_admin_emits_an_admin_transferred_event_with_old_and_new_admin() {
    let h = setup();
    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);

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
    h.client.deposit_legacy(&depositor, &7, &400);

    assert!(!h.client.is_paused());
    h.client.pause();
    assert!(h.client.is_paused());

    h.client.unpause();
    assert!(!h.client.is_paused());

    // Both sides work again post-unpause.
    h.client.deposit_legacy(&depositor, &7, &100);
    h.client.withdraw_legacy(&7, &depositor, &50);
    assert_eq!(h.client.balance_of_legacy(&7), 450);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // ContractPaused
fn deposit_is_rejected_while_paused() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.pause();
    h.client.deposit_legacy(&depositor, &7, &400);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // ContractPaused
fn withdraw_is_rejected_while_paused() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);

    h.client.pause();
    h.client.withdraw_legacy(&7, &depositor, &100);
}

// ─── events ──────────────────────────────────────────────────────────────────

#[test]
fn deposit_emits_a_deposited_event_with_the_amount_as_data() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit_legacy(&depositor, &7, &400);

    let events = h.env.events().all();
    let (contract_id, topics, data) = events.last().unwrap();
    assert_eq!(contract_id, h.client.address);
    let (data_from, amount) = <(Address, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(data_from, depositor);
    assert_eq!(amount, 400);

    // token and the full tag live in topics — an indexer filtering
    // "deposits of this token" or "deposits for this tag" reads them
    // from here.
    let topic_token = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let topic_tag = Tag::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(topic_token, h.token);
    assert_eq!(topic_tag, lt(&h, 7));
}

#[test]
fn withdraw_emits_a_withdrawn_event_with_the_amount_as_data() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);

    h.client.withdraw_legacy(&7, &depositor, &150);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    let (data_to, amount) = <(Address, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(data_to, depositor);
    assert_eq!(amount, 150);

    let topic_token = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let topic_tag = Tag::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(topic_token, h.token);
    assert_eq!(topic_tag, lt(&h, 7));
}

// ─── sweep_untagged ─────────────────────────────────────────────────────────

#[test]
fn sweep_untagged_recovers_a_direct_transfer_that_bypassed_deposit() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);

    // Simulate a stray direct transfer straight to the vault's own
    // address, bypassing deposit() entirely — those funds aren't credited
    // to any tag.
    mint(&h, &h.client.address, 250);

    let rescuer = Address::generate(&h.env);
    let swept = h.client.sweep_untagged(&h.token, &rescuer);

    assert_eq!(swept, 250);
    assert_eq!(balance(&h, &rescuer), 250);
    // Tag 7's own escrowed balance must be completely untouched.
    assert_eq!(h.client.balance_of_legacy(&7), 400);
    assert_eq!(h.client.get_total_escrowed(&h.token), 400);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // NoUntaggedFunds
fn sweep_untagged_rejects_when_the_balance_exactly_matches_the_ledger() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);

    // No stray transfer this time — the vault's actual balance (400)
    // exactly matches get_total_escrowed (400), so there's nothing to
    // sweep.
    let rescuer = Address::generate(&h.env);
    h.client.sweep_untagged(&h.token, &rescuer);
}

#[test]
fn sweep_untagged_emits_a_swept_untagged_event() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);
    mint(&h, &h.client.address, 250);

    let rescuer = Address::generate(&h.env);
    h.client.sweep_untagged(&h.token, &rescuer);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 250);

    // token and to live in topics, not data.
    let topic_token = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let topic_to = Address::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(topic_token, h.token);
    assert_eq!(topic_to, rescuer);
}

// ─── transfer_tag ───────────────────────────────────────────────────────────

#[test]
fn transfer_tag_moves_escrow_without_any_token_movement() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &400);

    let vault_balance_before = balance(&h, &h.client.address);
    h.client.transfer_tag_legacy(&1, &2, &150);

    assert_eq!(h.client.balance_of_legacy(&1), 250);
    assert_eq!(h.client.balance_of_legacy(&2), 150);
    // TotalEscrowed unchanged — nothing entered or left the vault.
    assert_eq!(h.client.get_total_escrowed(&h.token), 400);
    assert_eq!(balance(&h, &h.client.address), vault_balance_before);
}

#[test]
fn transfer_tag_accumulates_into_an_already_funded_destination() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &400);
    h.client.deposit_legacy(&depositor, &2, &100);

    h.client.transfer_tag_legacy(&1, &2, &400);

    assert_eq!(h.client.balance_of_legacy(&1), 0);
    assert_eq!(h.client.balance_of_legacy(&2), 500);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn transfer_tag_rejects_more_than_the_source_tags_balance() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &100);

    h.client.transfer_tag_legacy(&1, &2, &101);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidAmount
fn transfer_tag_rejects_a_non_positive_amount() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &100);

    h.client.transfer_tag_legacy(&1, &2, &0);
}

#[test]
fn transfer_tag_emits_a_tag_transferred_event() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &400);

    h.client.transfer_tag_legacy(&1, &2, &150);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    let (from_tag, to_tag, amount) = <(Tag, Tag, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(from_tag, lt(&h, 1));
    assert_eq!(to_tag, lt(&h, 2));
    assert_eq!(amount, 150);

    let topic_token = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    assert_eq!(topic_token, h.token);
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
    h.client.deposit_legacy(&depositor, &7, &400);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 4);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let payee = Address::generate(&h.env);
    h.client
        .withdraw_via_multisig(&multisig_id, &action_id, &h.token, &lt(&h, 7), &payee, &150);

    assert_eq!(h.client.balance_of_legacy(&7), 250);
    assert_eq!(balance(&h, &payee), 150);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn withdraw_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);
    let (multisig_id, _signers) = setup_multisig(&h);

    let payee = Address::generate(&h.env);
    h.client.withdraw_via_multisig(
        &multisig_id,
        &aid(&h.env, 99),
        &h.token,
        &lt(&h, 7),
        &payee,
        &150,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn withdraw_via_multisig_still_enforces_the_tags_own_balance_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 5);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    // Approval authorizes WHO can call this, not draining more than
    // tag 7 actually has earmarked.
    let payee = Address::generate(&h.env);
    h.client
        .withdraw_via_multisig(&multisig_id, &action_id, &h.token, &lt(&h, 7), &payee, &401);
}

// ─── cross-contract: transfer_tag_via_multisig ─────────────────────────────

#[test]
fn transfer_tag_via_multisig_moves_escrow_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &400);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 6);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client.transfer_tag_via_multisig(
        &multisig_id,
        &action_id,
        &h.token,
        &lt(&h, 1),
        &lt(&h, 2),
        &150,
    );

    assert_eq!(h.client.balance_of_legacy(&1), 250);
    assert_eq!(h.client.balance_of_legacy(&2), 150);
    assert_eq!(h.client.get_total_escrowed(&h.token), 400); // unaffected by the reassignment
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn transfer_tag_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &400);
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client.transfer_tag_via_multisig(
        &multisig_id,
        &aid(&h.env, 99),
        &h.token,
        &lt(&h, 1),
        &lt(&h, 2),
        &150,
    );
}

// ─── cross-contract: sweep_untagged_via_multisig ───────────────────────────

#[test]
fn sweep_untagged_via_multisig_recovers_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &400);
    // A stray direct transfer that bypassed deposit() entirely.
    mint(&h, &h.client.address, 100);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 7);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let recovered_to = Address::generate(&h.env);
    let swept =
        h.client
            .sweep_untagged_via_multisig(&multisig_id, &action_id, &h.token, &recovered_to);

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
        .sweep_untagged_via_multisig(&multisig_id, &aid(&h.env, 99), &h.token, &recovered_to);
}

// ─── multi-token (#79) ──────────────────────────────────────────────────────

#[test]
fn tokens_are_isolated_per_tag_and_per_total() {
    let h = setup();
    let other = new_token(&h);
    h.client.set_token_allowed(&other, &true);
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    token::StellarAssetClient::new(&h.env, &other).mint(&depositor, &1_000);
    let t = lt(&h, 7);

    h.client.deposit(&depositor, &h.token, &t, &400);
    h.client.deposit(&depositor, &other, &t, &300);
    h.client.withdraw(&other, &t, &depositor, &300);

    assert_eq!(h.client.balance_of(&h.token, &t), 400);
    assert_eq!(h.client.balance_of(&other, &t), 0);
    assert_eq!(h.client.get_total_escrowed(&h.token), 400);
    assert_eq!(h.client.get_total_escrowed(&other), 0);
    assert_eq!(balance(&h, &h.client.address), 400);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn withdrawing_token_a_never_touches_token_b() {
    let h = setup();
    let other = new_token(&h);
    h.client.set_token_allowed(&other, &true);
    let depositor = Address::generate(&h.env);
    token::StellarAssetClient::new(&h.env, &other).mint(&depositor, &1_000);
    h.client.deposit(&depositor, &other, &lt(&h, 7), &300);

    h.client.withdraw(&h.token, &lt(&h, 7), &depositor, &1);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // TokenNotAllowed
fn deposit_rejects_a_token_not_on_the_allowlist() {
    let h = setup();
    let other = new_token(&h);
    let depositor = Address::generate(&h.env);
    token::StellarAssetClient::new(&h.env, &other).mint(&depositor, &1_000);
    h.client.deposit(&depositor, &other, &lt(&h, 7), &300);
}

#[test]
fn removing_a_token_from_the_allowlist_keeps_existing_balances_withdrawable() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &400);

    h.client.set_token_allowed(&h.token, &false);
    assert!(!h.client.is_token_allowed(&h.token));
    h.client.withdraw_legacy(&7, &depositor, &400);
    assert_eq!(balance(&h, &depositor), 1_000);
}

// ─── namespaced tags (#83) ──────────────────────────────────────────────────

#[test]
fn two_kinds_with_the_same_id_are_isolated() {
    let h = setup();
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    let series = tag(&integrator, "series", 7, &h.env);
    let position = tag(&integrator, "position", 7, &h.env);
    mint(&h, &integrator, 1_000);

    h.client.deposit(&integrator, &h.token, &series, &400);
    h.client.deposit(&integrator, &h.token, &position, &100);

    assert_eq!(h.client.balance_of(&h.token, &series), 400);
    assert_eq!(h.client.balance_of(&h.token, &position), 100);
    // ...and neither aliases the legacy tag with the same id.
    assert_eq!(h.client.balance_of_legacy(&7), 0);
}

#[test]
fn two_integrators_with_the_same_kind_and_id_are_isolated() {
    let h = setup();
    let a = Address::generate(&h.env);
    let b = Address::generate(&h.env);
    h.client.set_integrator(&a, &true);
    h.client.set_integrator(&b, &true);
    mint(&h, &a, 1_000);
    mint(&h, &b, 1_000);

    h.client
        .deposit(&a, &h.token, &tag(&a, "series", 7, &h.env), &400);
    h.client
        .deposit(&b, &h.token, &tag(&b, "series", 7, &h.env), &100);

    assert_eq!(
        h.client.balance_of(&h.token, &tag(&a, "series", 7, &h.env)),
        400
    );
    assert_eq!(
        h.client.balance_of(&h.token, &tag(&b, "series", 7, &h.env)),
        100
    );
}

// ─── tag ownership (#80) ────────────────────────────────────────────────────

#[test]
fn first_deposit_records_the_tag_owner() {
    let h = setup();
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    let t = tag(&integrator, "series", 1, &h.env);
    assert_eq!(h.client.get_tag_owner(&t), None);

    // An end user deposits into the integrator's tag; the integrator
    // still owns it.
    let user = Address::generate(&h.env);
    mint(&h, &user, 1_000);
    h.client.deposit(&user, &h.token, &t, &400);
    assert_eq!(h.client.get_tag_owner(&t), Some(integrator));
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // UnregisteredIntegrator
fn an_unregistered_integrator_cannot_create_tags() {
    let h = setup();
    let rogue = Address::generate(&h.env);
    mint(&h, &rogue, 1_000);
    h.client
        .deposit(&rogue, &h.token, &tag(&rogue, "series", 1, &h.env), &400);
}

#[test]
fn withdraw_requires_the_tag_owner_not_the_admin() {
    let h = setup();
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    mint(&h, &integrator, 1_000);
    let t = tag(&integrator, "series", 1, &h.env);
    h.client.deposit(&integrator, &h.token, &t, &400);

    h.client.withdraw(&h.token, &t, &integrator, &100);

    let auths = h.env.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!(auths[0].0, integrator);
}

#[test]
fn transfer_tag_across_owners_requires_both_owners() {
    let h = setup();
    let a = Address::generate(&h.env);
    let b = Address::generate(&h.env);
    h.client.set_integrator(&a, &true);
    h.client.set_integrator(&b, &true);
    mint(&h, &a, 1_000);
    mint(&h, &b, 1_000);
    let ta = tag(&a, "series", 1, &h.env);
    let tb = tag(&b, "series", 1, &h.env);
    h.client.deposit(&a, &h.token, &ta, &400);
    h.client.deposit(&b, &h.token, &tb, &100);

    h.client.transfer_tag(&h.token, &ta, &tb, &150);

    let auths = h.env.auths();
    assert!(auths.iter().any(|(signer, _)| *signer == a));
    assert!(auths.iter().any(|(signer, _)| *signer == b));
    assert_eq!(h.client.balance_of(&h.token, &tb), 250);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn withdraw_from_a_never_created_tag_panics() {
    let h = setup();
    let integrator = Address::generate(&h.env);
    h.client.withdraw(
        &h.token,
        &tag(&integrator, "series", 1, &h.env),
        &integrator,
        &1,
    );
}

// ─── balance-delta accounting / shortfall (#82) ─────────────────────────────

/// Minimal token that burns 10% of every transfer, to exercise
/// balance-delta crediting in deposit().
mod fee_token {
    use soroban_sdk::{contract, contractimpl, Address, Env};

    #[contract]
    pub struct FeeToken;

    #[contractimpl]
    impl FeeToken {
        pub fn mint(env: Env, to: Address, amount: i128) {
            let b: i128 = env.storage().instance().get(&to).unwrap_or(0);
            env.storage().instance().set(&to, &(b + amount));
        }

        pub fn balance(env: Env, id: Address) -> i128 {
            env.storage().instance().get(&id).unwrap_or(0)
        }

        pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
            from.require_auth();
            let fb: i128 = env.storage().instance().get(&from).unwrap_or(0);
            let tb: i128 = env.storage().instance().get(&to).unwrap_or(0);
            env.storage().instance().set(&from, &(fb - amount));
            env.storage()
                .instance()
                .set(&to, &(tb + amount - amount / 10));
        }
    }
}

#[test]
fn deposit_credits_the_measured_delta_for_a_fee_on_transfer_token() {
    let h = setup();
    let fee_token_id = h.env.register_contract(None, fee_token::FeeToken);
    let fee_client = fee_token::FeeTokenClient::new(&h.env, &fee_token_id);
    h.client.set_token_allowed(&fee_token_id, &true);
    let depositor = Address::generate(&h.env);
    fee_client.mint(&depositor, &1_000);

    let credited = h
        .client
        .deposit(&depositor, &fee_token_id, &lt(&h, 7), &400);

    assert_eq!(credited, 360);
    assert_eq!(h.client.balance_of(&fee_token_id, &lt(&h, 7)), 360);
    assert_eq!(h.client.get_total_escrowed(&fee_token_id), 360);
    assert_eq!(h.client.get_shortfall(&fee_token_id), 0);
}

/// A clawback-enabled SAC, with 400 deposited under legacy tag 7 and
/// then 150 clawed back from the vault's own balance.
fn setup_clawed_back(h: &Harness) -> Address {
    let token_admin = Address::generate(&h.env);
    let sac = h.env.register_stellar_asset_contract_v2(token_admin);
    sac.issuer()
        .set_flag(soroban_sdk::testutils::IssuerFlags::RevocableFlag);
    sac.issuer()
        .set_flag(soroban_sdk::testutils::IssuerFlags::ClawbackEnabledFlag);
    let clawback_token = sac.address();
    h.client.set_token_allowed(&clawback_token, &true);

    let depositor = Address::generate(&h.env);
    let admin_client = token::StellarAssetClient::new(&h.env, &clawback_token);
    admin_client.mint(&depositor, &1_000);
    h.client
        .deposit(&depositor, &clawback_token, &lt(h, 7), &400);
    admin_client.clawback(&h.client.address, &150);
    clawback_token
}

#[test]
fn get_shortfall_reports_a_clawback() {
    let h = setup();
    let t = setup_clawed_back(&h);
    assert_eq!(h.client.get_shortfall(&t), 150);
    assert_eq!(h.client.get_shortfall(&h.token), 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // NoUntaggedFunds, not an arithmetic trap
fn sweep_untagged_returns_no_untagged_funds_on_a_shortfall() {
    let h = setup();
    let t = setup_clawed_back(&h);
    h.client.sweep_untagged(&t, &h.admin);
}

#[test]
fn withdraw_emits_shortfall_detected_when_the_ledger_is_underbacked() {
    let h = setup();
    let t = setup_clawed_back(&h);
    let payee = Address::generate(&h.env);

    h.client.withdraw(&t, &lt(&h, 7), &payee, &100);

    let events = h.env.events().all();
    let shortfall_event = events
        .iter()
        .find(|(_, topics, _)| {
            Symbol::try_from_val(&h.env, &topics.get(0).unwrap()).unwrap()
                == Symbol::new(&h.env, "shortfall_detected")
        })
        .unwrap();
    assert_eq!(i128::try_from_val(&h.env, &shortfall_event.2).unwrap(), 150);
    assert_eq!(h.client.balance_of(&t, &lt(&h, 7)), 300);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // InsufficientVaultBalance
fn withdraw_beyond_the_actual_balance_fails_with_a_clear_error() {
    let h = setup();
    let t = setup_clawed_back(&h);
    h.client.withdraw(&t, &lt(&h, 7), &h.admin, &400);
}

// ─── legacy migration ───────────────────────────────────────────────────────

#[test]
fn migrate_legacy_moves_old_u64_entries_into_the_admins_legacy_namespace() {
    use crate::types::LegacyKey;

    let h = setup();
    // Simulate a pre-upgrade vault: bare-u64 escrow entries and a single
    // TotalEscrowed, backed by real tokens.
    mint(&h, &h.client.address, 500);
    h.env.as_contract(&h.client.address, || {
        h.env
            .storage()
            .persistent()
            .set(&LegacyKey::Escrow(1), &200i128);
        h.env
            .storage()
            .persistent()
            .set(&LegacyKey::Escrow(2), &300i128);
        h.env
            .storage()
            .instance()
            .set(&LegacyKey::TotalEscrowed, &500i128);
    });

    h.client.migrate_legacy(&vec![&h.env, 1u64, 2u64, 3u64]);

    assert_eq!(h.client.balance_of_legacy(&1), 200);
    assert_eq!(h.client.balance_of_legacy(&2), 300);
    assert_eq!(h.client.get_total_escrowed(&h.token), 500);
    assert_eq!(h.client.get_tag_owner(&lt(&h, 1)), Some(h.admin.clone()));
    h.env.as_contract(&h.client.address, || {
        assert!(!h.env.storage().persistent().has(&LegacyKey::Escrow(1)));
        assert!(!h.env.storage().instance().has(&LegacyKey::TotalEscrowed));
    });

    // Migrated balances are withdrawable through the legacy wrappers.
    let payee = Address::generate(&h.env);
    h.client.withdraw_legacy(&2, &payee, &300);
    assert_eq!(balance(&h, &payee), 300);
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
    h.client.deposit_legacy(&depositor, &7, &1_000);
    for _ in 0..5 {
        advance_ledgers(&h.env, 20 * ttl::DAY_IN_LEDGERS);
        h.client.bump(&soroban_sdk::vec![
            &h.env,
            DataKey::Escrow(h.token.clone(), lt(&h, 7))
        ]);
    }
    assert!(is_live(
        &h.env,
        &h.client.address,
        Some(DataKey::Escrow(h.token.clone(), lt(&h, 7)))
    ));
    assert_eq!(h.client.balance_of_legacy(&7), 1_000);
}

#[test]
fn archived_tag_restores_and_withdraws_identically() {
    use crate::{ttl, types::DataKey};
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &1_000);
    for _ in 0..5 {
        advance_ledgers(&h.env, 20 * ttl::DAY_IN_LEDGERS);
        h.client.is_paused();
    }
    assert!(!is_live(
        &h.env,
        &h.client.address,
        Some(DataKey::Escrow(h.token.clone(), lt(&h, 7)))
    ));

    restore_archived(&h.env);

    assert_eq!(h.client.balance_of_legacy(&7), 1_000);
    let to = Address::generate(&h.env);
    h.client.withdraw_legacy(&7, &to, &400);
    assert_eq!(balance(&h, &to), 400);
    assert_eq!(h.client.balance_of_legacy(&7), 600);
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

// ─── storage schema versioning / migration ──────────────────────────────

fn contract_err(code: u32) -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(code)
}

/// Rewinds the instance to the v1 layout: no version stamp, no `Tokens`.
fn rewind_to_v1(h: &Harness) {
    use crate::types::DataKey;
    h.env.as_contract(&h.client.address, || {
        zenith_common::migrations::init_schema(&h.env, 1);
        h.env.storage().instance().remove(&DataKey::Tokens);
    });
}

/// Deposits under several tags of two integrators and two tokens, then
/// returns every `(token, tag, balance)` triple written.
fn seed_tags(h: &Harness) -> std::vec::Vec<(Address, Tag, i128)> {
    let other = new_token(h);
    h.client.set_token_allowed(&other, &true);
    token::StellarAssetClient::new(&h.env, &other).mint(&h.admin, &10_000);
    let a = Address::generate(&h.env);
    let b = Address::generate(&h.env);
    h.client.set_integrator(&a, &true);
    h.client.set_integrator(&b, &true);
    mint(h, &a, 10_000);
    mint(h, &b, 10_000);
    token::StellarAssetClient::new(&h.env, &other).mint(&a, &10_000);

    let mut out = std::vec::Vec::new();
    for (i, (who, tok, kind)) in [
        (&a, &h.token, "series"),
        (&a, &h.token, "position"),
        (&b, &h.token, "series"),
        (&a, &other, "series"),
    ]
    .into_iter()
    .enumerate()
    {
        let t = tag(who, kind, 7, &h.env);
        let amount = 100 * (i as i128 + 1);
        h.client.deposit(who, tok, &t, &amount);
        out.push((tok.clone(), t, amount));
    }
    out
}

#[test]
fn initialize_stamps_current_schema_version() {
    let h = setup();
    assert_eq!(
        h.client.version(),
        crate::migrations::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(h.client.version(), h.client.schema_version());
    assert_eq!(h.client.get_tokens(), vec![&h.env, h.token.clone()]);
}

#[test]
fn migrate_v1_to_v2_adds_token_index_and_unblocks_writes() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &500);
    rewind_to_v1(&h);
    assert_eq!(h.client.version(), 1);

    // A stale layout blocks every ledger write until migrated.
    assert_eq!(
        h.client.try_deposit_legacy(&depositor, &1, &100),
        Err(Ok(contract_err(102)))
    );

    assert_eq!(h.client.migrate(&1, &2), 2);
    assert_eq!(h.client.version(), 2);
    assert_eq!(h.client.get_tokens(), vec![&h.env, h.token.clone()]);
    assert_eq!(h.client.balance_of_legacy(&1), 500);
    h.client.deposit_legacy(&depositor, &1, &100);
    assert_eq!(h.client.balance_of_legacy(&1), 600);
}

#[test]
fn migrate_rejects_wrong_expected_from() {
    let h = setup();
    rewind_to_v1(&h);
    assert_eq!(h.client.try_migrate(&2, &2), Err(Ok(contract_err(100))));
    assert_eq!(h.client.try_migrate(&0, &2), Err(Ok(contract_err(100))));
    assert_eq!(h.client.version(), 1);
}

#[test]
fn repeated_migrate_returns_typed_error_without_reapplying() {
    let h = setup();
    rewind_to_v1(&h);
    assert_eq!(h.client.migrate(&1, &2), 2);
    let events_before = h.env.events().all().len();
    assert_eq!(h.client.try_migrate(&1, &2), Err(Ok(contract_err(100))));
    assert_eq!(h.client.version(), 2);
    // The failed retry rolled back and emitted nothing new.
    assert!(h.env.events().all().len() <= events_before);
}

#[test]
fn migrate_rejects_unsupported_target() {
    let h = setup();
    rewind_to_v1(&h);
    let next = crate::migrations::CURRENT_SCHEMA_VERSION + 1;
    assert_eq!(h.client.try_migrate(&1, &next), Err(Ok(contract_err(101))));
    assert_eq!(h.client.try_migrate(&1, &1), Err(Ok(contract_err(101))));
    assert_eq!(h.client.version(), 1);
}

#[test]
fn newer_than_supported_stored_version_is_refused() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    let future = crate::migrations::CURRENT_SCHEMA_VERSION + 1;
    h.env.as_contract(&h.client.address, || {
        zenith_common::migrations::init_schema(&h.env, future);
    });
    assert_eq!(
        h.client.try_deposit_legacy(&depositor, &1, &100),
        Err(Ok(contract_err(102)))
    );
    assert_eq!(
        h.client.try_migrate(&future, &(future + 1)),
        Err(Ok(contract_err(101)))
    );
}

#[test]
fn migrate_is_admin_gated() {
    let h = setup();
    rewind_to_v1(&h);
    h.env.mock_auths(&[]);
    assert!(h.client.try_migrate(&1, &2).is_err());
    assert_eq!(h.client.version(), 1);
}

#[test]
fn migrate_runs_while_paused() {
    let h = setup();
    rewind_to_v1(&h);
    h.client.pause();
    assert_eq!(h.client.migrate(&1, &2), 2);
    assert!(h.client.is_paused());
}

#[test]
fn migration_preserves_every_tag_balance_and_isolation() {
    let h = setup();
    let seeded = seed_tags(&h);
    let totals: std::vec::Vec<i128> = h
        .client
        .get_tokens()
        .iter()
        .map(|t| h.client.get_total_escrowed(&t))
        .collect();
    rewind_to_v1(&h);

    assert_eq!(h.client.migrate(&1, &2), 2);

    for (tok, t, amount) in &seeded {
        assert_eq!(h.client.balance_of(tok, t), *amount);
    }
    let after: std::vec::Vec<i128> = h
        .client
        .get_tokens()
        .iter()
        .map(|t| h.client.get_total_escrowed(&t))
        .collect();
    assert_eq!(after[0], totals[0]);
    // The second token rejoins the index on its next ledger write.
    let (tok, t, amount) = seeded.last().unwrap();
    assert!(!h.client.get_tokens().contains(tok));
    h.client.withdraw(tok, t, &h.admin, &(amount / 2));
    assert!(h.client.get_tokens().contains(tok));
    assert_eq!(h.client.get_total_escrowed(tok), totals[1] - amount / 2);

    // Tag isolation: a tag still can't pay out more than its own escrow,
    // even though the vault holds other tags' funds in the same token.
    let (tok0, t0, a0) = &seeded[0];
    let (_, t2, a2) = &seeded[2];
    assert!(h
        .client
        .try_withdraw(tok0, t2, &h.admin, &(a2 + 1))
        .is_err());
    assert_eq!(h.client.balance_of(tok0, t0), *a0);
    assert_eq!(h.client.balance_of(tok0, t2), *a2);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn migration_that_changes_a_ledger_total_aborts() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &1, &500);
    h.env.as_contract(&h.client.address, || {
        crate::migrations::preserving_ledger(&h.env, || {
            h.env.storage().instance().set(
                &crate::types::DataKey::TotalEscrowed(h.token.clone()),
                &1i128,
            );
            2
        });
    });
}
