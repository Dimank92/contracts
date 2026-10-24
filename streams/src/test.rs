#![cfg(test)]

use crate::{Streams, StreamsClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

struct Harness<'a> {
    env: Env,
    client: StreamsClient<'a>,
    token: token::Client<'a>,
    governance: Address,
    funder: Address,
    recipient: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);

    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let governance = Address::generate(&env);
    let funder = Address::generate(&env);
    let recipient = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token_address).mint(&funder, &1_000_000_000);

    let contract_id = env.register_contract(None, Streams);
    let client = StreamsClient::new(&env, &contract_id);
    client.initialize(&governance);

    Harness {
        token: token::Client::new(&env, &token_address),
        env,
        client,
        governance,
        funder,
        recipient,
    }
}

impl Harness<'_> {
    fn create(&self, total: i128, start: u64, cliff: u64, end: u64) -> u64 {
        self.client.create_stream(
            &self.funder,
            &self.recipient,
            &self.token.address,
            &total,
            &start,
            &cliff,
            &end,
        )
    }

    fn warp(&self, t: u64) {
        self.env.ledger().set_timestamp(t);
    }
}

#[test]
fn create_pulls_deposit_upfront() {
    let h = setup();
    let id = h.create(1_000, 1_000, 1_000, 2_000);
    assert_eq!(id, 0);
    assert_eq!(h.token.balance(&h.client.address), 1_000);
    assert_eq!(h.client.get_stream_count(), 1);
    assert_eq!(h.client.balance_of(&id), (0, 1_000));
}

#[test]
fn accrues_linearly_over_time() {
    let h = setup();
    let id = h.create(1_000, 1_000, 1_000, 2_000);
    h.warp(1_250);
    assert_eq!(h.client.balance_of(&id), (250, 750));
    h.client.withdraw(&id, &200);
    assert_eq!(h.token.balance(&h.recipient), 200);
    assert_eq!(h.client.balance_of(&id), (50, 750));
    h.warp(5_000);
    assert_eq!(h.client.balance_of(&id), (800, 0));
    h.client.withdraw(&id, &800);
    assert_eq!(h.token.balance(&h.recipient), 1_000);
    assert_eq!(h.token.balance(&h.client.address), 0);
}

#[test]
fn nothing_withdrawable_before_cliff() {
    let h = setup();
    let id = h.create(1_000, 1_000, 1_500, 2_000);
    h.warp(1_499);
    assert_eq!(h.client.balance_of(&id), (0, 1_000));
    assert!(h.client.try_withdraw(&id, &1).is_err());
    h.warp(1_500);
    assert_eq!(h.client.balance_of(&id), (500, 500));
}

#[test]
fn start_in_the_past_accrues_immediately() {
    let h = setup();
    h.warp(1_500);
    let id = h.create(1_000, 1_000, 1_000, 2_000);
    assert_eq!(h.client.balance_of(&id), (500, 500));
}

#[test]
fn rejects_invalid_schedules_and_amounts() {
    let h = setup();
    let t = &h.token.address;
    let (f, r) = (&h.funder, &h.recipient);
    // zero-duration
    assert!(h
        .client
        .try_create_stream(f, r, t, &1_000, &1_000, &1_000, &1_000)
        .is_err());
    // cliff before start / after end
    assert!(h
        .client
        .try_create_stream(f, r, t, &1_000, &1_000, &999, &2_000)
        .is_err());
    assert!(h
        .client
        .try_create_stream(f, r, t, &1_000, &1_000, &2_001, &2_000)
        .is_err());
    // non-positive total
    assert!(h
        .client
        .try_create_stream(f, r, t, &0, &1_000, &1_000, &2_000)
        .is_err());
}

#[test]
fn cancel_splits_accrued_and_remainder() {
    let h = setup();
    let id = h.create(1_000, 1_000, 1_000, 2_000);
    h.warp(1_300);
    h.client.withdraw(&id, &100);
    let funder_before = h.token.balance(&h.funder);
    h.client.cancel(&h.funder, &id);
    assert_eq!(h.token.balance(&h.recipient), 300);
    assert_eq!(h.token.balance(&h.funder) - funder_before, 700);
    assert_eq!(h.token.balance(&h.client.address), 0);
    assert_eq!(h.client.balance_of(&id), (0, 0));
    // frozen: time passing accrues nothing more
    h.warp(3_000);
    assert_eq!(h.client.balance_of(&id), (0, 0));
    assert!(h.client.try_cancel(&h.funder, &id).is_err());
}

#[test]
fn cancel_before_cliff_refunds_everything() {
    let h = setup();
    let id = h.create(1_000, 1_000, 1_800, 2_000);
    h.warp(1_700);
    let funder_before = h.token.balance(&h.funder);
    h.client.cancel(&h.governance, &id);
    assert_eq!(h.token.balance(&h.recipient), 0);
    assert_eq!(h.token.balance(&h.funder) - funder_before, 1_000);
}

#[test]
fn only_funder_or_governance_can_cancel() {
    let h = setup();
    let id = h.create(1_000, 1_000, 1_000, 2_000);
    let stranger = Address::generate(&h.env);
    assert!(h.client.try_cancel(&stranger, &id).is_err());
    assert!(h.client.try_cancel(&h.recipient, &id).is_err());
}

#[test]
fn recipient_can_transfer_stream() {
    let h = setup();
    let id = h.create(1_000, 1_000, 1_000, 2_000);
    h.warp(1_500);
    let new_recipient = Address::generate(&h.env);
    h.client.transfer(&id, &new_recipient);
    h.client.withdraw(&id, &500);
    assert_eq!(h.token.balance(&new_recipient), 500);
    assert_eq!(h.token.balance(&h.recipient), 0);
}

/// Property test: over many pseudo-random schedules, totals, and
/// withdrawal/cancel sequences, the total paid out never exceeds the
/// deposit and everything deposited is eventually accounted for.
#[test]
fn prop_withdrawn_never_exceeds_deposit() {
    let h = setup();
    h.env.budget().reset_unlimited();
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = |m: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % m
    };

    for _ in 0..25 {
        let now = h.env.ledger().timestamp();
        let start = now - next(500);
        let end = start + 1 + next(10_000);
        let cliff = start + next(end - start + 1);
        let total = 1 + next(1_000_000) as i128;
        let recipient_before = h.token.balance(&h.recipient);
        let funder_before = h.token.balance(&h.funder);
        let id = h.create(total, start, cliff, end);

        let mut paid = 0i128;
        for _ in 0..8 {
            h.warp(h.env.ledger().timestamp() + next(2_000));
            let (avail, _) = h.client.balance_of(&id);
            if avail > 0 {
                let amt = 1 + next(avail as u64) as i128;
                h.client.withdraw(&id, &amt);
                paid += amt;
            }
            assert!(paid <= total);
            assert!(h.client.try_withdraw(&id, &(avail + 1)).is_err());
        }
        if next(2) == 0 {
            h.client.cancel(&h.funder, &id);
        } else {
            h.warp(end);
            let (avail, refundable) = h.client.balance_of(&id);
            assert_eq!(refundable, 0);
            if avail > 0 {
                h.client.withdraw(&id, &avail);
            }
        }
        let to_recipient = h.token.balance(&h.recipient) - recipient_before;
        let to_funder = h.token.balance(&h.funder) - funder_before + total;
        assert!(to_recipient <= total);
        assert_eq!(to_recipient + to_funder, total);
        assert_eq!(h.token.balance(&h.client.address), 0);
    }
}

#[test]
fn second_initialize_is_rejected_and_keeps_governance() {
    let h = setup();
    let attacker = Address::generate(&h.env);
    let res = h.client.try_initialize(&attacker);
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            crate::error::Error::AlreadyInitialized as u32
        )))
    );
    assert_eq!(h.client.get_governance(), h.governance);
}

#[test]
fn legacy_deployment_without_flag_is_treated_as_initialized() {
    let h = setup();
    h.env.as_contract(&h.client.address, || {
        h.env
            .storage()
            .instance()
            .remove(&crate::types::DataKey::Initialized);
    });
    let res = h.client.try_initialize(&Address::generate(&h.env));
    assert_eq!(
        res,
        Err(Ok(soroban_sdk::Error::from_contract_error(
            crate::error::Error::AlreadyInitialized as u32
        )))
    );
}
