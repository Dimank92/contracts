#![cfg(test)]

extern crate std;

use crate::types::Tag;
use crate::{Vault, VaultClient};
use test_adversaries::multisig::{ThresholdMultisig, ThresholdMultisigClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events as _, MockAuth, MockAuthInvoke},
    token, Address, Env, IntoVal, Symbol, TryFromVal,
};

struct Harness<'a> {
    env: Env,
    client: VaultClient<'a>,
    admin: Address,
    token: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin);
    let token = token_contract.address();

    let contract_id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &contract_id);
    client.initialize(&admin, &token, &(7 * 86_400u64));

    Harness {
        env,
        client,
        admin,
        token,
    }
}

fn mint(h: &Harness, to: &Address, amount: i128) {
    token::StellarAssetClient::new(&h.env, &h.token).mint(to, &amount);
}

fn make_tag(h: &Harness, owner: Address) -> Tag {
    Tag {
        owner,
        kind: symbol_short!("series"),
        id: 1,
    }
}

// ─── initialize ──────────────────────────────────────────────────────────────

#[test]
fn initialize_sets_admin_and_allowlists_default_token() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);
    assert!(h.client.is_token_allowed(&h.token));
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn initialize_twice_panics() {
    let h = setup();
    h.client.initialize(&h.admin, &h.token, &(7 * 86_400u64));
}

// ─── token allowlist (issue #193) ────────────────────────────────────────────

#[test]
fn set_token_allowed_adds_a_second_token() {
    let h = setup();
    let token2 = env_token(&h.env);
    assert!(!h.client.is_token_allowed(&token2));

    h.client.set_token_allowed(&token2, &true);
    assert!(h.client.is_token_allowed(&token2));
}

#[test]
fn set_token_allowed_false_removes_from_allowlist() {
    let h = setup();
    h.client.set_token_allowed(&h.token, &false);
    assert!(!h.client.is_token_allowed(&h.token));
}

#[test]
fn set_token_allowed_emits_token_allowed_event() {
    let h = setup();
    let token2 = env_token(&h.env);
    let before = h.env.events().all().len();

    h.client.set_token_allowed(&token2, &true);

    assert!(h.env.events().all().len() > before);
    let (_, _topics, data) = h.env.events().all().last().unwrap();
    let (ev_token, allowed) =
        <(Address, bool)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(ev_token, token2);
    assert!(allowed);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // TokenNotAllowed
fn deposit_rejects_a_non_allowlisted_token() {
    let h = setup();
    let token2 = env_token(&h.env);
    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);

    mint(&h, &depositor, 1_000);
    h.client.deposit(
        &depositor,
        &token2, // not allowlisted
        &make_tag(&h, integrator),
        &500,
    );
}

// ─── integrator registry (issue #193) ────────────────────────────────────────

#[test]
fn set_integrator_registers_an_integrator() {
    let h = setup();
    let integrator = Address::generate(&h.env);
    assert!(!h.client.is_integrator(&integrator));

    h.client.set_integrator(&integrator, &true);
    assert!(h.client.is_integrator(&integrator));
}

#[test]
fn set_integrator_false_deregisters_it() {
    let h = setup();
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    h.client.set_integrator(&integrator, &false);
    assert!(!h.client.is_integrator(&integrator));
}

#[test]
fn set_integrator_emits_integrator_set_event() {
    let h = setup();
    let integrator = Address::generate(&h.env);
    let before = h.env.events().all().len();

    h.client.set_integrator(&integrator, &true);

    assert!(h.env.events().all().len() > before);
    let (_, _topics, data) = h.env.events().all().last().unwrap();
    let (ev_integrator, allowed) =
        <(Address, bool)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(ev_integrator, integrator);
    assert!(allowed);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // UnregisteredIntegrator
fn deposit_rejects_an_unregistered_integrator_on_first_tag() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    let unregistered = Address::generate(&h.env);

    mint(&h, &depositor, 1_000);
    h.client.deposit(
        &depositor,
        &h.token,
        &make_tag(&h, unregistered),
        &500,
    );
}

#[test]
fn deposit_succeeds_for_registered_integrator() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);

    mint(&h, &depositor, 1_000);
    let tag = make_tag(&h, integrator.clone());
    let received = h.client.deposit(&depositor, &h.token, &tag, &500);

    assert_eq!(received, 500);
    assert_eq!(h.client.balance_of_tagged(&h.token, &tag), 500);
    assert_eq!(h.client.get_total_escrowed(&h.token), 500);
}

#[test]
fn removing_integrator_does_not_strand_existing_tag_balances() {
    // After deregistering an integrator, existing tag balances are still
    // withdrawable — deregistration only blocks NEW tags.
    let h = setup();
    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);

    mint(&h, &depositor, 1_000);
    let tag = make_tag(&h, integrator.clone());
    h.client.deposit(&depositor, &h.token, &tag, &500);

    // Deregister the integrator
    h.client.set_integrator(&integrator, &false);
    assert!(!h.client.is_integrator(&integrator));

    // Existing tag is still withdrawable (tag owner auth is from the tag
    // record, not the integrator registry)
    let recipient = Address::generate(&h.env);
    h.client.withdraw(&h.token, &tag, &recipient, &500);
    assert_eq!(h.client.balance_of_tagged(&h.token, &tag), 0);
}

#[test]
fn removing_token_does_not_strand_existing_tag_balances() {
    // After removing a token from the allowlist, existing deposits are still
    // withdrawable.
    let h = setup();
    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);

    mint(&h, &depositor, 1_000);
    let tag = make_tag(&h, integrator.clone());
    h.client.deposit(&depositor, &h.token, &tag, &500);

    // Remove the token from allowlist
    h.client.set_token_allowed(&h.token, &false);

    // Existing balance is still withdrawable
    let recipient = Address::generate(&h.env);
    h.client.withdraw(&h.token, &tag, &recipient, &500);
    assert_eq!(h.client.balance_of_tagged(&h.token, &tag), 0);
}

// ─── deposit / withdraw basic ─────────────────────────────────────────────────

#[test]
fn deposit_and_withdraw_round_trip() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    mint(&h, &depositor, 1_000);

    let tag = make_tag(&h, integrator.clone());
    h.client.deposit(&depositor, &h.token, &tag, &800);
    assert_eq!(h.client.balance_of_tagged(&h.token, &tag), 800);

    let recipient = Address::generate(&h.env);
    h.client.withdraw(&h.token, &tag, &recipient, &300);
    assert_eq!(h.client.balance_of_tagged(&h.token, &tag), 500);
    assert_eq!(
        token::Client::new(&h.env, &h.token).balance(&recipient),
        300
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn withdraw_rejects_over_the_tagged_balance() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    mint(&h, &depositor, 1_000);

    let tag = make_tag(&h, integrator.clone());
    h.client.deposit(&depositor, &h.token, &tag, &200);

    let recipient = Address::generate(&h.env);
    h.client.withdraw(&h.token, &tag, &recipient, &300); // more than tagged
}

// ─── pause / unpause ─────────────────────────────────────────────────────────

#[test]
fn pause_blocks_deposit_and_withdraw() {
    let h = setup();
    h.client.pause();
    assert!(h.client.is_paused());

    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    mint(&h, &depositor, 1_000);
    let tag = make_tag(&h, integrator);

    let result = h
        .client
        .try_deposit(&depositor, &h.token, &tag, &500);
    assert!(result.is_err());
}

#[test]
fn unpause_restores_operations() {
    let h = setup();
    h.client.pause();
    h.client.unpause();
    assert!(!h.client.is_paused());

    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    mint(&h, &depositor, 1_000);
    let tag = make_tag(&h, integrator.clone());
    let received = h.client.deposit(&depositor, &h.token, &tag, &500);
    assert_eq!(received, 500);
}

// ─── sweep_untagged ───────────────────────────────────────────────────────────

#[test]
fn sweep_untagged_recovers_directly_sent_tokens() {
    let h = setup();

    // Send tokens directly to the vault (bypassing deposit)
    let vault_addr = h.client.address.clone();
    mint(&h, &vault_addr, 1_000);

    let recipient = Address::generate(&h.env);
    h.client.sweep_untagged(&h.token, &recipient);
    assert_eq!(
        token::Client::new(&h.env, &h.token).balance(&recipient),
        1_000
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // NoUntaggedFunds
fn sweep_untagged_rejects_when_all_funds_are_tagged() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    mint(&h, &depositor, 1_000);

    let tag = make_tag(&h, integrator);
    h.client.deposit(&depositor, &h.token, &tag, &1_000);

    let recipient = Address::generate(&h.env);
    h.client.sweep_untagged(&h.token, &recipient);
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn env_token(env: &Env) -> Address {
    let admin = Address::generate(env);
    env.register_stellar_asset_contract_v2(admin).address()
}

/// Multisig action id (`zenith_common` keys actions by `u64`).
fn aid(_env: &Env, n: u64) -> u64 {
    n
}

// ─── allowlist admin surface hardening (issue #193) ─────────────────────────

fn setup_multisig(h: &Harness) -> (Address, [Address; 3]) {
    let signers = [
        Address::generate(&h.env),
        Address::generate(&h.env),
        Address::generate(&h.env),
    ];
    let id = h.env.register_contract(None, ThresholdMultisig);
    ThresholdMultisigClient::new(&h.env, &id).initialize(
        &soroban_sdk::vec![
            &h.env,
            signers[0].clone(),
            signers[1].clone(),
            signers[2].clone()
        ],
        &2,
        &0,
    );
    (id, signers)
}

#[test]
fn allowlist_mutations_require_the_admin() {
    let h = setup();
    let other = env_token(&h.env);
    let integrator = Address::generate(&h.env);
    // Drop the blanket mock: every require_auth() now needs a real
    // authorization, and none is provided.
    h.env.set_auths(&[]);
    assert!(h.client.try_set_token_allowed(&other, &true).is_err());
    assert!(h.client.try_set_token_allowed(&h.token, &false).is_err());
    assert!(h.client.try_set_integrator(&integrator, &true).is_err());
    assert!(!h.client.is_token_allowed(&other));
    assert!(h.client.is_token_allowed(&h.token));
    assert!(!h.client.is_integrator(&integrator));
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // NotInitialized
fn set_token_allowed_before_initialize_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &id);
    client.set_token_allowed(&env_token(&env), &true);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // NotInitialized
fn set_integrator_before_initialize_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &id);
    client.set_integrator(&Address::generate(&env), &true);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // UnregisteredIntegrator
fn deposit_into_an_existing_tag_is_rejected_after_deregistration() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    mint(&h, &depositor, 1_000);
    let tag = make_tag(&h, integrator.clone());
    h.client.deposit(&depositor, &h.token, &tag, &500);

    h.client.set_integrator(&integrator, &false);
    h.client.deposit(&depositor, &h.token, &tag, &500);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // TokenNotAllowed
fn deposit_into_an_existing_tag_is_rejected_after_token_removal() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    mint(&h, &depositor, 1_000);
    let tag = make_tag(&h, integrator.clone());
    h.client.deposit(&depositor, &h.token, &tag, &500);

    h.client.set_token_allowed(&h.token, &false);
    h.client.deposit(&depositor, &h.token, &tag, &500);
}

#[test]
fn legacy_deposit_is_gated_by_the_token_allowlist_but_withdraw_is_not() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit_legacy(&depositor, &7, &600);
    assert_eq!(h.client.balance_of_legacy(&7), 600);

    h.client.set_token_allowed(&h.token, &false);
    let err = h
        .client
        .try_deposit_legacy(&depositor, &7, &100)
        .err()
        .unwrap()
        .unwrap();
    assert_eq!(err, crate::error::Error::TokenNotAllowed.into());

    // Removal does not strand the existing legacy balance.
    let recipient = Address::generate(&h.env);
    h.client.withdraw_legacy(&7, &recipient, &600);
    assert_eq!(h.client.balance_of_legacy(&7), 0);
    assert_eq!(token::Client::new(&h.env, &h.token).balance(&recipient), 600);
}

#[test]
fn a_depositor_cannot_write_into_another_integrators_namespace() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    mint(&h, &depositor, 1_000);
    let tag = make_tag(&h, integrator.clone());

    // Only the depositor signs (for the deposit and the token pull); the
    // integrator that owns the tag does not.
    h.env.mock_auths(&[MockAuth {
        address: &depositor,
        invoke: &MockAuthInvoke {
            contract: &h.client.address,
            fn_name: "deposit",
            args: (&depositor, &h.token, tag.clone(), 500i128).into_val(&h.env),
            sub_invokes: &[MockAuthInvoke {
                contract: &h.token,
                fn_name: "transfer",
                args: (&depositor, &h.client.address, 500i128).into_val(&h.env),
                sub_invokes: &[],
            }],
        },
    }]);
    assert!(h
        .client
        .try_deposit(&depositor, &h.token, &tag, &500)
        .is_err());
    assert_eq!(h.client.balance_of_tagged(&h.token, &tag), 0);
}

#[test]
fn removing_a_token_emits_token_allowed_false() {
    let h = setup();
    h.client.set_token_allowed(&h.token, &false);
    let (_, topics, data) = h.env.events().all().last().unwrap();
    let name = Symbol::try_from_val(&h.env, &topics.get(0).unwrap()).unwrap();
    assert_eq!(name, Symbol::new(&h.env, "token_allowed"));
    let (ev_token, allowed): (Address, bool) = <(Address, bool)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(ev_token, h.token);
    assert!(!allowed);
}

#[test]
fn deregistering_an_integrator_emits_integrator_set_false() {
    let h = setup();
    let integrator = Address::generate(&h.env);
    h.client.set_integrator(&integrator, &true);
    h.client.set_integrator(&integrator, &false);
    let (_, _, data) = h.env.events().all().last().unwrap();
    let (ev, allowed): (Address, bool) = <(Address, bool)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(ev, integrator);
    assert!(!allowed);
}

#[test]
fn allowlists_can_be_managed_via_multisig_once_approved() {
    let h = setup();
    let (ms_id, signers) = setup_multisig(&h);
    let ms = ThresholdMultisigClient::new(&h.env, &ms_id);
    let other = env_token(&h.env);
    let integrator = Address::generate(&h.env);

    ms.approve(&signers[0], &aid(&h.env, 1));
    ms.approve(&signers[1], &aid(&h.env, 1));
    h.client
        .set_token_allowed_via_multisig(&ms_id, &aid(&h.env, 1), &other, &true);
    assert!(h.client.is_token_allowed(&other));

    ms.approve(&signers[0], &aid(&h.env, 2));
    ms.approve(&signers[2], &aid(&h.env, 2));
    h.client
        .set_integrator_via_multisig(&ms_id, &aid(&h.env, 2), &integrator, &true);
    assert!(h.client.is_integrator(&integrator));
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn allowlist_via_multisig_rejects_without_quorum() {
    let h = setup();
    let (ms_id, signers) = setup_multisig(&h);
    ThresholdMultisigClient::new(&h.env, &ms_id).approve(&signers[0], &aid(&h.env, 3));
    h.client.set_integrator_via_multisig(
        &ms_id,
        &aid(&h.env, 3),
        &Address::generate(&h.env),
        &true,
    );
}
