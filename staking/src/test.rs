#![cfg(test)]

use crate::{Error, Staking, StakingClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    token, Address, Env, IntoVal, Symbol,
};

const COOLDOWN: u64 = 7 * 86_400;
const MAX_REWARD_RATE: u32 = 10_000;

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

    /// Returns true if the most recent event batch contains an event emitted
    /// by the staking contract with the given topic symbol.
    fn emitted(&self, topic: &str) -> bool {
        let sym = Symbol::new(&self.env, topic);
        self.env.events().all().iter().any(|(id, topics, _)| {
            id == self.client.address
                && topics.len() == 1
                && topics.get(0).unwrap() == sym.into_val(&self.env)
        })
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

    assert_eq!(h.client.try_withdraw(&a), Err(Ok(Error::CooldownActive)));
    h.env.ledger().set_timestamp(COOLDOWN);
    assert_eq!(h.client.withdraw(&a), 100);
    assert_eq!(h.zen.balance(&a), 100);
    assert_eq!(h.client.try_withdraw(&a), Err(Ok(Error::NothingToWithdraw)));
}

#[test]
fn cannot_unstake_more_than_staked() {
    let h = setup();
    let a = h.staker(100);
    assert_eq!(
        h.client.try_request_unstake(&a, &101),
        Err(Ok(Error::InsufficientStake))
    );
    assert_eq!(
        h.client.try_request_unstake(&a, &0),
        Err(Ok(Error::InvalidAmount))
    );
}

#[test]
fn only_splitter_can_notify() {
    let h = setup();
    h.staker(100);
    h.env.set_auths(&[]);
    // No splitter signature: the host rejects the auth check.
    assert!(matches!(
        h.client.try_notify_reward(&h.usdc.address, &1_000),
        Err(Err(_))
    ));
}

#[test]
fn reward_token_cap_and_reinitialize_are_typed_errors() {
    let h = setup();
    h.staker(100);
    for _ in 1..crate::MAX_REWARD_TOKENS {
        let t = new_token(&h.env);
        mint(&h.env, &t, &h.splitter, 1);
        h.client.notify_reward(&t.address, &1);
    }
    h.client.notify_reward(&h.usdc.address, &1);
    let extra = new_token(&h.env);
    mint(&h.env, &extra, &h.splitter, 1);
    assert_eq!(
        h.client.try_notify_reward(&extra.address, &1),
        Err(Ok(Error::TooManyRewardTokens))
    );
    assert_eq!(
        h.client
            .try_initialize(&h.zen.address, &h.splitter, &COOLDOWN),
        Err(Ok(Error::AlreadyInitialized))
    );
}

#[test]
fn uninitialized_contract_reports_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let client = StakingClient::new(&env, &env.register_contract(None, Staking));
    assert_eq!(client.try_get_splitter(), Err(Ok(Error::NotInitialized)));
    assert_eq!(
        client.try_stake(&Address::generate(&env), &1),
        Err(Ok(Error::NotInitialized))
    );
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

// ---------------------------------------------------------------------------
// Admin controls: reward rate, cooldown, emergency unstake
// ---------------------------------------------------------------------------

#[test]
fn set_reward_rate_updates_getter_and_emits_event() {
    let h = setup();
    assert_eq!(h.client.reward_rate(), 0);
    h.client.set_reward_rate(&500);
    assert_eq!(h.client.reward_rate(), 500);
    assert!(h.emitted("reward_rate_set"));
}

#[test]
fn set_reward_rate_allows_zero_explicitly() {
    let h = setup();
    h.client.set_reward_rate(&1_000);
    h.client.set_reward_rate(&0);
    assert_eq!(h.client.reward_rate(), 0);
}

#[test]
fn set_reward_rate_allows_maximum() {
    let h = setup();
    h.client.set_reward_rate(&MAX_REWARD_RATE);
    assert_eq!(h.client.reward_rate(), MAX_REWARD_RATE);
}

#[test]
fn set_reward_rate_rejects_out_of_range() {
    let h = setup();
    assert!(h.client.try_set_reward_rate(&(MAX_REWARD_RATE + 1)).is_err());
    assert_eq!(h.client.reward_rate(), 0);
}

#[test]
fn set_reward_rate_requires_admin() {
    let h = setup();
    h.env.set_auths(&[]);
    assert!(h.client.try_set_reward_rate(&500).is_err());
    assert_eq!(h.client.reward_rate(), 0);
}

#[test]
fn set_cooldown_updates_getter_and_emits_event() {
    let h = setup();
    assert_eq!(h.client.cooldown(), COOLDOWN);
    h.client.set_cooldown(&(COOLDOWN * 2));
    assert_eq!(h.client.cooldown(), COOLDOWN * 2);
    assert!(h.emitted("cooldown_set"));
}

#[test]
fn set_cooldown_requires_admin() {
    let h = setup();
    h.env.set_auths(&[]);
    assert!(h.client.try_set_cooldown(&1).is_err());
    assert_eq!(h.client.cooldown(), COOLDOWN);
}

#[test]
fn cooldown_change_does_not_affect_in_flight_unbonds() {
    let h = setup();
    let a = h.staker(100);
    h.client.request_unstake(&a, &100);
    // Shorten the cooldown after the unbond was initiated.
    h.client.set_cooldown(&1);
    assert_eq!(h.client.cooldown(), 1);
    // The existing unbond keeps its original unlock time.
    h.env.ledger().set_timestamp(COOLDOWN - 1);
    assert!(h.client.try_withdraw(&a).is_err());
    h.env.ledger().set_timestamp(COOLDOWN);
    assert_eq!(h.client.withdraw(&a), 100);
    assert_eq!(h.zen.balance(&a), 100);
}

#[test]
fn cooldown_lengthening_does_not_delay_in_flight_unbonds() {
    let h = setup();
    let a = h.staker(100);
    h.client.request_unstake(&a, &100);
    h.client.set_cooldown(&(COOLDOWN * 10));
    h.env.ledger().set_timestamp(COOLDOWN);
    assert_eq!(h.client.withdraw(&a), 100);
}

#[test]
fn emergency_unstake_lets_staker_exit_immediately() {
    let h = setup();
    let a = h.staker(100);
    h.client.set_emergency_unstake(&true);
    assert!(h.client.emergency_unstake_enabled());
    assert!(h.emitted("emergency_unstake_set"));
    assert_eq!(h.client.emergency_unstake(&a), 100);
    assert_eq!(h.zen.balance(&a), 100);
    // No double withdrawal.
    assert!(h.client.try_emergency_unstake(&a).is_err());
    assert_eq!(h.zen.balance(&a), 100);
}

#[test]
fn emergency_unstake_blocks_new_stakes() {
    let h = setup();
    h.client.set_emergency_unstake(&true);
    let user = Address::generate(&h.env);
    mint(&h.env, &h.zen, &user, 100);
    assert!(h.client.try_stake(&user, &100).is_err());
}

#[test]
fn emergency_unstake_disabled_by_default() {
    let h = setup();
    assert!(!h.client.emergency_unstake_enabled());
}

#[test]
fn set_emergency_unstake_requires_admin() {
    let h = setup();
    h.env.set_auths(&[]);
    assert!(h.client.try_set_emergency_unstake(&true).is_err());
    assert!(!h.client.emergency_unstake_enabled());
}
