#![cfg(test)]

use crate::{Staking, StakingClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env,
};

/// Advance the test ledger's timestamp by `delta` seconds.
fn advance_time(env: &Env, delta: u64) {
    let now = env.ledger().timestamp();
    env.ledger().set_timestamp(now + delta);
}

const COOLDOWN: u64 = 7 * 86_400;

struct Harness<'a> {
    env: Env,
    client: StakingClient<'a>,
    zen: token::Client<'a>,
    usdc: token::Client<'a>,
    splitter: Address,
}

fn new_token<'a>(env: &Env) -> token::Client<'a> {
    let address = env
        .register_stellar_asset_contract_v2(Address::generate(env))
        .address();
    token::Client::new(env, &address)
}

fn mint(env: &Env, token: &token::Client, to: &Address, amount: i128) {
    token::StellarAssetClient::new(env, &token.address).mint(to, &amount);
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let zen = new_token(&env);
    let usdc = new_token(&env);
    let splitter = Address::generate(&env);
    mint(&env, &usdc, &splitter, i64::MAX as i128);

    let contract_id = env.register_contract(None, Staking);
    let client = StakingClient::new(&env, &contract_id);
    client.initialize(&zen.address, &splitter, &COOLDOWN);
    Harness {
        env,
        client,
        zen,
        usdc,
        splitter,
    }
}

impl Harness<'_> {
    fn staker(&self, amount: i128) -> Address {
        let user = Address::generate(&self.env);
        mint(&self.env, &self.zen, &user, amount);
        if amount > 0 {
            self.client.stake(&user, &amount);
        }
        user
    }
}

#[test]
fn rewards_split_pro_rata() {
    let h = setup();
    let a = h.staker(100);
    let b = h.staker(300);
    h.client.notify_reward(&h.usdc.address, &1_000);
    assert_eq!(h.client.earned(&a, &h.usdc.address), 250);
    assert_eq!(h.client.earned(&b, &h.usdc.address), 750);
    h.client.claim_rewards(&a);
    assert_eq!(h.usdc.balance(&a), 250);
    assert_eq!(h.client.earned(&a, &h.usdc.address), 0);
}

#[test]
fn late_staker_gets_nothing_from_earlier_rewards() {
    let h = setup();
    let a = h.staker(100);
    h.client.notify_reward(&h.usdc.address, &1_000);
    let b = h.staker(100);
    h.client.notify_reward(&h.usdc.address, &1_000);
    assert_eq!(h.client.earned(&a, &h.usdc.address), 1_500);
    assert_eq!(h.client.earned(&b, &h.usdc.address), 500);
}

#[test]
fn rewards_with_nothing_staked_carry_over() {
    let h = setup();
    h.client.notify_reward(&h.usdc.address, &1_000);
    assert_eq!(h.client.undistributed(&h.usdc.address), 1_000);
    let a = h.staker(50);
    assert_eq!(h.client.undistributed(&h.usdc.address), 0);
    assert_eq!(h.client.earned(&a, &h.usdc.address), 1_000);
}

#[test]
fn multiple_reward_tokens() {
    let h = setup();
    let other = new_token(&h.env);
    mint(&h.env, &other, &h.splitter, 10_000);
    let a = h.staker(1);
    let b = h.staker(1);
    h.client.notify_reward(&h.usdc.address, &100);
    h.client.notify_reward(&other.address, &10);
    assert_eq!(h.client.reward_tokens().len(), 2);
    h.client.claim_rewards(&a);
    h.client.claim_rewards(&b);
    assert_eq!((h.usdc.balance(&a), other.balance(&a)), (50, 5));
    assert_eq!((h.usdc.balance(&b), other.balance(&b)), (50, 5));
}

#[test]
fn unstake_respects_cooldown_and_stops_earning() {
    let h = setup();
    let a = h.staker(100);
    let b = h.staker(100);
    h.client.request_unstake(&a, &100);
    assert_eq!(h.client.total_staked(), 100);
    h.client.notify_reward(&h.usdc.address, &1_000);
    assert_eq!(h.client.earned(&a, &h.usdc.address), 0);
    assert_eq!(h.client.earned(&b, &h.usdc.address), 1_000);

    assert!(h.client.try_withdraw(&a).is_err());
    h.env.ledger().set_timestamp(COOLDOWN);
    assert_eq!(h.client.withdraw(&a), 100);
    assert_eq!(h.zen.balance(&a), 100);
    assert!(h.client.try_withdraw(&a).is_err());
}

#[test]
fn cannot_unstake_more_than_staked() {
    let h = setup();
    let a = h.staker(100);
    assert!(h.client.try_request_unstake(&a, &101).is_err());
}

#[test]
fn only_splitter_can_notify() {
    let h = setup();
    h.staker(100);
    h.env.set_auths(&[]);
    assert!(h.client.try_notify_reward(&h.usdc.address, &1_000).is_err());
}

#[test]
fn tiny_stake_next_to_huge_stake_keeps_precision() {
    let h = setup();
    let whale = h.staker(1_000_000_000_000_000); // 1e8 tokens at 7 decimals
    let minnow = h.staker(1);
    h.client
        .notify_reward(&h.usdc.address, &1_000_000_000_000_000);
    assert_eq!(h.client.earned(&minnow, &h.usdc.address), 0);
    h.client
        .notify_reward(&h.usdc.address, &1_000_000_000_000_000_000);
    assert_eq!(h.client.earned(&minnow, &h.usdc.address), 1_000);
    assert!(h.client.earned(&whale, &h.usdc.address) < 1_001_000_000_000_000_000);
}

/// Property test: random multi-staker simulation. No staker ever accrues
/// more than their exact pro-rata share, and total claimed plus what is
/// still owed never exceeds what was notified.
#[test]
fn prop_no_staker_claims_more_than_share() {
    let h = setup();
    h.env.budget().reset_unlimited();
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = |m: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % m
    };

    let users: [Address; 4] = core::array::from_fn(|_| h.staker(0));
    for u in users.iter() {
        mint(&h.env, &h.zen, u, 1_000_000_000);
    }
    let mut stakes = [0i128; 4];
    // Upper bound on each user's entitlement: sum of ceil(amount × s / total).
    let mut bound = [0i128; 4];
    let mut notified = 0i128;

    for _ in 0..60 {
        let i = next(4) as usize;
        match next(4) {
            0 => {
                let amt = 1 + next(10_000_000) as i128;
                h.client.stake(&users[i], &amt);
                stakes[i] += amt;
            }
            1 if stakes[i] > 0 => {
                let amt = 1 + next(stakes[i] as u64) as i128;
                h.client.request_unstake(&users[i], &amt);
                stakes[i] -= amt;
            }
            2 => h.client.claim_rewards(&users[i]),
            _ => {
                let total: i128 = stakes.iter().sum();
                if total == 0 {
                    continue;
                }
                let amt = 1 + next(1_000_000_000) as i128;
                h.client.notify_reward(&h.usdc.address, &amt);
                notified += amt;
                for j in 0..4 {
                    bound[j] += (amt * stakes[j] + total - 1) / total;
                }
            }
        }
        let mut accounted = 0i128;
        for j in 0..4 {
            let entitled = h.usdc.balance(&users[j]) + h.client.earned(&users[j], &h.usdc.address);
            assert!(entitled <= bound[j]);
            accounted += entitled;
        }
        assert!(accounted <= notified);
        assert!(h.usdc.balance(&h.client.address) >= notified - accounted);
    }
}

#[test]
fn query_empty_state() {
    let h = setup();
    let unknown = Address::generate(&h.env);

    let pos = h.client.get_position(&unknown);
    assert_eq!(pos.staked, 0);
    assert_eq!(pos.pending_unstake, 0);
    assert_eq!(pos.unstake_ready_at, 0);
    assert_eq!(h.client.earned(&unknown, &h.usdc.address), 0);
    assert_eq!(h.client.total_staked(), 0);

    let page = h.client.list_positions(&0, &10);
    assert_eq!(page.len(), 0);
}

#[test]
fn query_single_and_multiple_stakers() {
    let h = setup();
    let a = h.staker(100);
    let b = h.staker(250);

    let pos_a = h.client.get_position(&a);
    assert_eq!(pos_a.staked, 100);
    assert_eq!(pos_a.pending_unstake, 0);
    assert_eq!(h.client.get_position(&b).staked, 250);
    assert_eq!(h.client.total_staked(), 350);

    let page = h.client.list_positions(&0, &10);
    assert_eq!(page.len(), 2);
    let mut seen = 0i128;
    for entry in page.iter() {
        seen += entry.position.staked;
    }
    assert_eq!(seen, 350);
}

#[test]
fn query_accrued_rewards_before_and_after_time_advance() {
    let h = setup();
    let a = h.staker(100);
    let b = h.staker(300);
    h.client.notify_reward(&h.usdc.address, &1_000);

    assert_eq!(h.client.earned(&a, &h.usdc.address), 250);
    assert_eq!(h.client.earned(&b, &h.usdc.address), 750);

    advance_time(&h.env, 1_000);
    // Accrual is driven by the reward accumulator, not elapsed time, so the
    // computed view is stable across ledger-time advancement.
    assert_eq!(h.client.earned(&a, &h.usdc.address), 250);
    assert_eq!(h.client.earned(&b, &h.usdc.address), 750);
}

#[test]
fn query_claim_then_earned_is_zero() {
    let h = setup();
    let a = h.staker(100);
    h.client.notify_reward(&h.usdc.address, &1_000);
    assert_eq!(h.client.earned(&a, &h.usdc.address), 1_000);

    h.client.claim_rewards(&a);
    assert_eq!(h.client.earned(&a, &h.usdc.address), 0);
    assert_eq!(h.usdc.balance(&a), 1_000);
    // Position is unchanged by claiming.
    assert_eq!(h.client.get_position(&a).staked, 100);
}

#[test]
fn query_reward_config() {
    let h = setup();
    let cfg = h.client.reward_config();
    assert_eq!(cfg.cooldown, COOLDOWN);
    assert_eq!(cfg.splitter, h.splitter);
}

#[test]
fn query_pagination_boundaries() {
    let h = setup();
    for _ in 0..5 {
        h.staker(10);
    }

    let first = h.client.list_positions(&0, &2);
    assert_eq!(first.len(), 2);

    let last = h.client.list_positions(&4, &2);
    assert_eq!(last.len(), 1);

    let past = h.client.list_positions(&5, &2);
    assert_eq!(past.len(), 0);

    let past_far = h.client.list_positions(&1_000, &2);
    assert_eq!(past_far.len(), 0);
}

#[test]
fn query_pagination_rejects_bad_limits() {
    let h = setup();
    h.staker(10);
    assert!(h.client.try_list_positions(&0, &0).is_err());
    assert!(h
        .client
        .try_list_positions(&0, &(crate::MAX_PAGE_SIZE + 1))
        .is_err());
}
