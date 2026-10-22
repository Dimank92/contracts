#![cfg(test)]

use crate::{Schedule, Streams, StreamsClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env, Vec,
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

    fn advance(&self, delta: u64) {
        let now = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(now + delta);
    }

    fn create_linear_with_cliff(
        &self,
        total: i128,
        start: u64,
        cliff: u64,
        end: u64,
    ) -> u64 {
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

    fn create_stepped(
        &self,
        total: i128,
        start: u64,
        cliff: u64,
        end: u64,
        checkpoints: Vec<(u64, i128)>,
    ) -> u64 {
        self.client.create_stream_stepped(
            &self.funder,
            &self.recipient,
            &self.token.address,
            &total,
            &start,
            &cliff,
            &end,
            &checkpoints,
        )
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

#[test]
fn linear_with_cliff_boundaries() {
    let h = setup();
    let id = h.create_linear_with_cliff(1_000, 1_000, 1_500, 2_000);
    // before the cliff: exactly zero, even though time has passed
    h.warp(1_000);
    assert_eq!(h.client.balance_of(&id), (0, 1_000));
    h.warp(1_499);
    assert_eq!(h.client.balance_of(&id), (0, 1_000));
    // exactly at the cliff: accrues from stream start, not from cliff
    h.warp(1_500);
    assert_eq!(h.client.balance_of(&id), (500, 500));
    // between cliff and end: continuous from start
    h.warp(1_750);
    assert_eq!(h.client.balance_of(&id), (750, 250));
    // at the end: fully vested
    h.warp(2_000);
    assert_eq!(h.client.balance_of(&id), (1_000, 0));
    // after the end: still fully vested, never more than total
    h.warp(9_999);
    assert_eq!(h.client.balance_of(&id), (1_000, 0));
}

#[test]
fn linear_with_cliff_equal_to_start() {
    let h = setup();
    let id = h.create_linear_with_cliff(1_000, 1_000, 1_000, 2_000);
    h.warp(1_000);
    assert_eq!(h.client.balance_of(&id), (0, 1_000));
    h.warp(1_500);
    assert_eq!(h.client.balance_of(&id), (500, 500));
    h.warp(2_000);
    assert_eq!(h.client.balance_of(&id), (1_000, 0));
}

#[test]
fn stepped_boundaries() {
    let h = setup();
    let mut cps = Vec::new(&h.env);
    cps.push_back((1_000u64, 0i128));
    cps.push_back((1_500u64, 250i128));
    cps.push_back((1_750u64, 600i128));
    cps.push_back((2_000u64, 1_000i128));
    let id = h.create_stepped(1_000, 1_000, 1_000, 2_000, cps);

    // before the cliff (and before first checkpoint): zero
    h.warp(999);
    assert_eq!(h.client.balance_of(&id), (0, 1_000));
    // exactly at the cliff / first checkpoint
    h.warp(1_000);
    assert_eq!(h.client.balance_of(&id), (0, 1_000));
    // between checkpoints: holds the last released cumulative amount
    h.warp(1_499);
    assert_eq!(h.client.balance_of(&id), (0, 1_000));
    h.warp(1_500);
    assert_eq!(h.client.balance_of(&id), (250, 750));
    h.warp(1_749);
    assert_eq!(h.client.balance_of(&id), (250, 750));
    h.warp(1_750);
    assert_eq!(h.client.balance_of(&id), (600, 400));
    // at the end: final cumulative equals total
    h.warp(2_000);
    assert_eq!(h.client.balance_of(&id), (1_000, 0));
    // after the end: capped at total
    h.warp(50_000);
    assert_eq!(h.client.balance_of(&id), (1_000, 0));
}

#[test]
fn stepped_with_cliff_after_start() {
    let h = setup();
    let mut cps = Vec::new(&h.env);
    cps.push_back((1_000u64, 0i128));
    cps.push_back((1_800u64, 400i128));
    cps.push_back((2_000u64, 1_000i128));
    let id = h.create_stepped(1_000, 1_000, 1_800, 2_000, cps);
    h.warp(1_799);
    assert_eq!(h.client.balance_of(&id), (0, 1_000));
    h.warp(1_800);
    assert_eq!(h.client.balance_of(&id), (400, 600));
    h.warp(2_000);
    assert_eq!(h.client.balance_of(&id), (1_000, 0));
}

#[test]
fn rejects_cliff_after_end() {
    let h = setup();
    let t = &h.token.address;
    let (f, r) = (&h.funder, &h.recipient);
    assert!(h
        .client
        .try_create_stream(f, r, t, &1_000, &1_000, &2_001, &2_000)
        .is_err());
    let mut cps = Vec::new(&h.env);
    cps.push_back((1_000u64, 0i128));
    cps.push_back((2_000u64, 1_000i128));
    assert!(h
        .client
        .try_create_stream_stepped(f, r, t, &1_000, &1_000, &2_001, &2_000, &cps)
        .is_err());
}

#[test]
fn rejects_duplicate_checkpoint_timestamps() {
    let h = setup();
    let t = &h.token.address;
    let (f, r) = (&h.funder, &h.recipient);
    let mut cps = Vec::new(&h.env);
    cps.push_back((1_000u64, 0i128));
    cps.push_back((1_500u64, 300i128));
    cps.push_back((1_500u64, 600i128));
    cps.push_back((2_000u64, 1_000i128));
    assert!(h
        .client
        .try_create_stream_stepped(f, r, t, &1_000, &1_000, &1_000, &2_000, &cps)
        .is_err());
}

#[test]
fn rejects_curve_exceeding_total() {
    let h = setup();
    let t = &h.token.address;
    let (f, r) = (&h.funder, &h.recipient);
    // cumulative amount exceeds total
    let mut cps = Vec::new(&h.env);
    cps.push_back((1_000u64, 0i128));
    cps.push_back((1_500u64, 1_500i128));
    cps.push_back((2_000u64, 1_000i128));
    assert!(h
        .client
        .try_create_stream_stepped(f, r, t, &1_000, &1_000, &1_000, &2_000, &cps)
        .is_err());
    // final cumulative does not equal total
    let mut cps2 = Vec::new(&h.env);
    cps2.push_back((1_000u64, 0i128));
    cps2.push_back((2_000u64, 999i128));
    assert!(h
        .client
        .try_create_stream_stepped(f, r, t, &1_000, &1_000, &1_000, &2_000, &cps2)
        .is_err());
    // non-monotonic cumulative amounts
    let mut cps3 = Vec::new(&h.env);
    cps3.push_back((1_000u64, 0i128));
    cps3.push_back((1_500u64, 600i128));
    cps3.push_back((1_750u64, 300i128));
    cps3.push_back((2_000u64, 1_000i128));
    assert!(h
        .client
        .try_create_stream_stepped(f, r, t, &1_000, &1_000, &1_000, &2_000, &cps3)
        .is_err());
}

#[test]
fn rejects_too_many_checkpoints() {
    let h = setup();
    let t = &h.token.address;
    let (f, r) = (&h.funder, &h.recipient);
    let cap = crate::MAX_CHECKPOINTS;
    let mut cps = Vec::new(&h.env);
    // one more than the cap
    for i in 0..=cap {
        cps.push_back((1_000u64 + i as u64, i as i128));
    }
    assert!(h
        .client
        .try_create_stream_stepped(
            f,
            r,
            t,
            &(cap as i128),
            &1_000,
            &1_000,
            &(1_000 + cap as u64),
            &cps,
        )
        .is_err());
}

/// Property test: withdrawable amount never decreases as time advances,
/// for both linear-with-cliff and stepped schedules.
#[test]
fn prop_withdrawable_never_decreases() {
    let h = setup();
    h.env.budget().reset_unlimited();
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = |m: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % m
    };

    for _ in 0..20 {
        let start = 1_000 + next(500);
        let end = start + 1 + next(5_000);
        let cliff = start + next(end - start + 1);
        let total = 1 + next(1_000_000) as i128;

        // linear-with-cliff
        let id = h.create_linear_with_cliff(total, start, cliff, end);
        let mut last = 0i128;
        let mut t = start;
        while t <= end + 500 {
            h.warp(t);
            let (avail, _) = h.client.balance_of(&id);
            assert!(avail >= last, "linear withdrawable decreased");
            assert!(avail <= total);
            last = avail;
            t += 1 + next(250);
        }

        // stepped with a strictly increasing random curve ending at total
        let steps = 1 + next(8);
        let mut cps = Vec::new(&h.env);
        cps.push_back((start, 0i128));
        let mut prev_t = start;
        let mut prev_amt = 0i128;
        for i in 0..steps {
            let remaining_t = end - prev_t;
            let remaining_amt = total - prev_amt;
            let dt = if i + 1 == steps {
                remaining_t
            } else {
                1 + next(remaining_t.max(1))
            };
            let da = if i + 1 == steps {
                remaining_amt
            } else {
                next(remaining_amt.max(1) as u64) as i128
            };
            prev_t += dt;
            prev_amt += da;
            cps.push_back((prev_t, prev_amt));
        }
        let sid = h.create_stepped(total, start, cliff, end, cps);
        let mut last = 0i128;
        let mut t = start;
        while t <= end + 500 {
            h.warp(t);
            let (avail, _) = h.client.balance_of(&sid);
            assert!(avail >= last, "stepped withdrawable decreased");
            assert!(avail <= total);
            last = avail;
            t += 1 + next(250);
        }
    }
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
