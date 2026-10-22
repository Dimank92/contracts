#![cfg(test)]

extern crate std;

use crate::error::Error;
use crate::{Tag, Vault, VaultClient};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events as _},
    token, vec, Address, Env, IntoVal, Symbol, TryFromVal, Val,
};
use test_adversaries::tokens::{
    BlacklistToken, BlacklistTokenClient, FeeOnTransferToken, LyingBalanceToken, NoMoveToken,
    NoMoveTokenClient, OverflowToken, ReentrantToken, ReentrantTokenClient, RevertOnTransferToken,
    ZeroBalanceToken,
};

/// Asserts a `try_*` call failed with exactly the contract error `e`.
fn expect_err<T: core::fmt::Debug, E: core::fmt::Debug>(
    res: Result<T, Result<soroban_sdk::Error, E>>,
    e: Error,
) {
    match res {
        Err(Ok(got)) => assert_eq!(got, soroban_sdk::Error::from(e)),
        other => panic!("expected {e:?}, got {other:?}"),
    }
}

struct Harness<'a> {
    env: Env,
    client: VaultClient<'a>,
    token: Address,
    integrator: Address,
    depositor: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let client = VaultClient::new(&env, &env.register_contract(None, Vault));
    client.initialize(&Address::generate(&env), &token, &3_600);
    let integrator = Address::generate(&env);
    client.set_integrator(&integrator, &true);
    let depositor = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token).mint(&depositor, &1_000_000);
    Harness {
        env,
        client,
        token,
        integrator,
        depositor,
    }
}

impl Harness<'_> {
    fn tag(&self, id: u64) -> Tag {
        Tag {
            owner: self.integrator.clone(),
            kind: symbol_short!("series"),
            id,
        }
    }

    fn deposit(&self, tag: &Tag, amount: i128) -> i128 {
        self.client
            .deposit(&self.depositor, &self.token, tag, &amount)
    }

    fn raw_balance(&self) -> i128 {
        token::Client::new(&self.env, &self.token).balance(&self.client.address)
    }

    fn allow(&self, token: &Address) {
        self.client.set_token_allowed(token, &true);
    }
}

// ── deposit / withdraw basics ───────────────────────────────────────────────

#[test]
fn deposit_credits_and_withdraw_pays_out() {
    let h = setup();
    let tag = h.tag(1);
    assert_eq!(h.deposit(&tag, 1_000), 1_000);
    assert_eq!(h.client.balance_of(&h.token, &tag), 1_000);
    assert_eq!(h.client.get_total_escrowed(&h.token), 1_000);
    let to = Address::generate(&h.env);
    h.client.withdraw(&h.token, &tag, &to, &400);
    assert_eq!(token::Client::new(&h.env, &h.token).balance(&to), 400);
    assert_eq!(h.client.balance_of(&h.token, &tag), 600);
    expect_err(
        h.client.try_withdraw(&h.token, &tag, &to, &601),
        Error::InsufficientEscrowBalance,
    );
}

#[test]
fn new_tags_need_a_registered_integrator() {
    let h = setup();
    h.client.set_integrator(&h.integrator, &false);
    expect_err(
        h.client.try_deposit(&h.depositor, &h.token, &h.tag(1), &10),
        Error::UnregisteredIntegrator,
    );
}

// ── transfer_tag (#265) ─────────────────────────────────────────────────────

#[test]
fn transfer_tag_moves_ledger_without_moving_tokens() {
    let h = setup();
    let (a, b) = (h.tag(1), h.tag(2));
    h.deposit(&a, 1_000);
    let raw = h.raw_balance();

    h.client.transfer_tag(&h.token, &a, &b, &300);

    assert_eq!(h.client.balance_of(&h.token, &a), 700);
    assert_eq!(h.client.balance_of(&h.token, &b), 300);
    assert_eq!(h.client.get_total_escrowed(&h.token), 1_000);
    assert_eq!(h.raw_balance(), raw);
    assert_eq!(h.client.get_tag_owner(&b), Some(h.integrator.clone()));

    let (_, topics, data): (Address, soroban_sdk::Vec<Val>, Val) =
        h.env.events().all().last().unwrap();
    assert_eq!(
        Symbol::try_from_val(&h.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&h.env, "tag_transferred")
    );
    let (f, t, n): (Tag, Tag, i128) = TryFromVal::try_from_val(&h.env, &data).unwrap();
    assert_eq!((f, t, n), (a, b, 300));
}

#[test]
fn transfer_tag_rejects_bad_requests_with_distinct_errors() {
    let h = setup();
    let (a, b) = (h.tag(1), h.tag(2));
    h.deposit(&a, 1_000);

    let t = &h.token;
    expect_err(
        h.client.try_transfer_tag(t, &a, &b, &1_001),
        Error::InsufficientEscrowBalance,
    );
    expect_err(h.client.try_transfer_tag(t, &a, &a, &1), Error::SameTag);
    expect_err(
        h.client.try_transfer_tag(t, &a, &b, &0),
        Error::InvalidAmount,
    );
    // unknown from_tag
    expect_err(
        h.client.try_transfer_tag(t, &h.tag(99), &b, &1),
        Error::UnknownTag,
    );
    // a different integrator's to_tag: out of scope, and its owner's auth
    // is not enough to pull from `a`
    let other = Tag {
        owner: Address::generate(&h.env),
        kind: symbol_short!("series"),
        id: 1,
    };
    expect_err(
        h.client.try_transfer_tag(t, &a, &other, &1),
        Error::CrossOwnerTransfer,
    );
    // retired integrator
    h.client.set_integrator(&h.integrator, &false);
    expect_err(
        h.client.try_transfer_tag(t, &a, &b, &1),
        Error::UnregisteredIntegrator,
    );
    assert_eq!(h.client.balance_of(t, &a), 1_000);
    assert_eq!(h.client.balance_of(t, &b), 0);
}

#[test]
fn transfer_tag_needs_the_from_tag_owner() {
    let h = setup();
    let (a, b) = (h.tag(1), h.tag(2));
    h.deposit(&a, 1_000);
    h.env.set_auths(&[]);
    assert!(h.client.try_transfer_tag(&h.token, &a, &b, &1).is_err());
    assert_eq!(h.client.balance_of(&h.token, &a), 1_000);
}

#[test]
fn transfer_tag_preserves_the_sum_of_tag_balances() {
    let h = setup();
    let tags = [h.tag(1), h.tag(2), h.tag(3)];
    for (i, t) in tags.iter().enumerate() {
        h.deposit(t, 1_000 * (i as i128 + 1));
    }
    let sum = |h: &Harness| -> i128 { tags.iter().map(|t| h.client.balance_of(&h.token, t)).sum() };
    let before = sum(&h);
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..50 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let (from, to) = (seed as usize % 3, (seed as usize / 3 + 1) % 3);
        let amount = 1 + (seed % 700) as i128;
        let _ = h
            .client
            .try_transfer_tag(&h.token, &tags[from], &tags[to], &amount);
        assert_eq!(sum(&h), before);
        assert_eq!(h.client.get_total_escrowed(&h.token), before);
    }
}

// ── adversarial tokens (#264) ───────────────────────────────────────────────

/// Tokens whose `transfer` cannot deliver the requested amount.
fn broken_inbound_tokens(env: &Env) -> std::vec::Vec<(&'static str, Address, Option<Error>)> {
    std::vec![
        // 10% burned in flight
        (
            "fee",
            env.register_contract(None, FeeOnTransferToken),
            Some(Error::TokenDeltaMismatch)
        ),
        // returns cleanly, moves nothing
        (
            "no-move",
            env.register_contract(None, NoMoveToken),
            Some(Error::TokenDeltaMismatch)
        ),
        // balance() stuck at 0
        (
            "zero-balance",
            env.register_contract(None, ZeroBalanceToken),
            Some(Error::TokenDeltaMismatch)
        ),
        // balance() stuck at i128::MAX: before == after
        (
            "lying",
            env.register_contract(None, LyingBalanceToken),
            Some(Error::TokenDeltaMismatch)
        ),
        (
            "overflow",
            env.register_contract(None, OverflowToken),
            Some(Error::TokenDeltaMismatch)
        ),
        // transfer always panics: the token's own contract error surfaces
        (
            "revert",
            env.register_contract(None, RevertOnTransferToken),
            None
        ),
    ]
}

#[test]
fn deposit_fails_closed_on_every_broken_token() {
    let h = setup();
    for (name, token, expected) in broken_inbound_tokens(&h.env) {
        h.allow(&token);
        let tag = h.tag(7);
        let res = h.client.try_deposit(&h.depositor, &token, &tag, &1_000);
        match expected {
            Some(e) => expect_err(res, e),
            None => assert!(res.is_err(), "{name}"),
        }
        // ledger untouched, nothing credited
        assert_eq!(h.client.balance_of(&token, &tag), 0, "{name}");
        assert_eq!(h.client.get_total_escrowed(&token), 0, "{name}");
    }
}

#[test]
fn withdraw_against_unfundable_tokens_cannot_conjure_balance() {
    let h = setup();
    let tag = h.tag(7);
    h.deposit(&tag, 500); // tag exists, but only in the honest token
    let to = Address::generate(&h.env);
    for (name, token, _) in broken_inbound_tokens(&h.env) {
        expect_err(
            h.client.try_withdraw(&token, &tag, &to, &1),
            Error::InsufficientEscrowBalance,
        );
        assert_eq!(h.client.get_total_escrowed(&token), 0, "{name}");
    }
    assert_eq!(h.client.balance_of(&h.token, &tag), 500);
}

#[test]
fn blacklisted_payout_reverts_and_leaves_the_ledger_intact() {
    let h = setup();
    let token = h.env.register_contract(None, BlacklistToken);
    let tc = BlacklistTokenClient::new(&h.env, &token);
    h.allow(&token);
    tc.mint(&h.depositor, &1_000);
    let tag = h.tag(3);
    assert_eq!(h.client.deposit(&h.depositor, &token, &tag, &1_000), 1_000);

    let to = Address::generate(&h.env);
    tc.set_blacklisted(&to, &true);
    assert!(h.client.try_withdraw(&token, &tag, &to, &400).is_err());
    assert_eq!(h.client.balance_of(&token, &tag), 1_000);
    assert_eq!(h.client.get_total_escrowed(&token), 1_000);
    assert_eq!(tc.balance(&h.client.address), 1_000);

    // blacklisted depositor: deposit reverts, nothing credited
    tc.set_blacklisted(&h.depositor, &true);
    assert!(h
        .client
        .try_deposit(&h.depositor, &token, &tag, &10)
        .is_err());
    assert_eq!(h.client.balance_of(&token, &tag), 1_000);

    tc.set_blacklisted(&to, &false);
    h.client.withdraw(&token, &tag, &to, &400);
    assert_eq!(tc.balance(&to), 400);
    assert_eq!(h.client.balance_of(&token, &tag), 600);
}

#[test]
fn no_move_token_cannot_be_credited_or_drained() {
    let h = setup();
    let token = h.env.register_contract(None, NoMoveToken);
    h.allow(&token);
    let tc = NoMoveTokenClient::new(&h.env, &token);
    tc.mint(&h.depositor, &100);
    let tag = h.tag(4);
    expect_err(
        h.client.try_deposit(&h.depositor, &token, &tag, &100),
        Error::TokenDeltaMismatch,
    );
    assert_eq!(tc.balance(&h.depositor), 100);
    assert_eq!(h.client.balance_of(&token, &tag), 0);
}

/// Soroban's host forbids a contract re-entering one already on the call
/// stack, so a token that calls back into the vault mid-transfer is
/// rejected by construction (the mock records the rejection); the deposit
/// still completes and credits exactly the amount received.
#[test]
fn reentrant_token_cannot_reenter_the_vault() {
    let h = setup();
    let token = h.env.register_contract(None, ReentrantToken);
    let tc = ReentrantTokenClient::new(&h.env, &token);
    h.allow(&token);
    tc.mint(&h.depositor, &1_000);
    let tag = h.tag(5);
    let args = vec![
        &h.env,
        token.clone().into_val(&h.env),
        tag.clone().into_val(&h.env),
        h.depositor.clone().into_val(&h.env),
        1_000i128.into_val(&h.env),
    ];
    tc.set_reentry_target(&h.client.address, &Symbol::new(&h.env, "withdraw"));
    tc.set_reentry_args(&args);

    assert_eq!(h.client.deposit(&h.depositor, &token, &tag, &1_000), 1_000);
    assert_eq!(tc.reentry_succeeded(), Some(false));
    assert_eq!(h.client.balance_of(&token, &tag), 1_000);
    assert_eq!(h.client.get_total_escrowed(&token), 1_000);

    // and on the withdraw path (the token is called from inside `withdraw`)
    h.client.withdraw(&token, &tag, &h.depositor, &400);
    assert_eq!(tc.reentry_succeeded(), Some(false));
    assert_eq!(h.client.balance_of(&token, &tag), 600);
    assert_eq!(tc.balance(&h.client.address), 600);
}
