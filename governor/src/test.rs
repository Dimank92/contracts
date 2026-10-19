#![cfg(test)]

use crate::test_timelock::{MockTimelock, MockTimelockClient};
use crate::{Error, Governor, GovernorClient, ProposalState};
use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{Address as _, Ledger as _},
    vec, Address, BytesN, Env, IntoVal, Symbol, Val, Vec,
};

const DELAY_LEDGERS: u32 = 10;
const PERIOD_LEDGERS: u32 = 720;
const QUORUM_BPS: u32 = 400; // 4%
const THRESHOLD: i128 = 1_000;
const TL_DELAY: u64 = 86_400;
const TL_GRACE: u64 = 7 * 86_400;

#[contracttype]
enum VotesKey {
    Account(Address),
    Supply,
}

/// Minimal checkpointed voting token standing in for gov_token.
#[contract]
pub struct MockVotes;

fn past(checkpoints: Vec<(u32, i128)>, ledger: u32) -> i128 {
    let mut value = 0;
    for (at, v) in checkpoints.iter() {
        if at <= ledger {
            value = v;
        }
    }
    value
}

fn push(env: &Env, key: VotesKey, delta: i128) {
    let mut cps: Vec<(u32, i128)> = env.storage().instance().get(&key).unwrap_or(vec![env]);
    let last = cps.last().map(|(_, v)| v).unwrap_or(0);
    cps.push_back((env.ledger().sequence(), last + delta));
    env.storage().instance().set(&key, &cps);
}

#[contractimpl]
impl MockVotes {
    pub fn adjust(env: Env, account: Address, delta: i128) {
        push(&env, VotesKey::Account(account), delta);
        push(&env, VotesKey::Supply, delta);
    }
    pub fn get_past_votes(env: Env, account: Address, ledger: u32) -> i128 {
        let cps = env.storage().instance().get(&VotesKey::Account(account));
        past(cps.unwrap_or(vec![&env]), ledger)
    }
    pub fn get_past_total_supply(env: Env, ledger: u32) -> i128 {
        let cps = env.storage().instance().get(&VotesKey::Supply);
        past(cps.unwrap_or(vec![&env]), ledger)
    }
}

struct Harness<'a> {
    env: Env,
    gov: GovernorClient<'a>,
    tl: MockTimelockClient<'a>,
    votes: MockVotesClient<'a>,
    proposer: Address,
    guardian: Address,
    alice: Address,
    bob: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_sequence_number(100);
    env.ledger().set_timestamp(1_000_000);

    let votes = MockVotesClient::new(&env, &env.register_contract(None, MockVotes));
    let gov_id = env.register_contract(None, Governor);
    let tl = MockTimelockClient::new(&env, &env.register_contract(None, MockTimelock));
    let guardian = Address::generate(&env);
    tl.initialize(&gov_id, &guardian, &TL_DELAY, &TL_GRACE);
    let gov = GovernorClient::new(&env, &gov_id);
    gov.initialize(
        &votes.address,
        &tl.address,
        &DELAY_LEDGERS,
        &PERIOD_LEDGERS,
        &QUORUM_BPS,
        &THRESHOLD,
    );

    let proposer = Address::generate(&env);
    let alice = Address::generate(&env);
    let bob = Address::generate(&env);
    votes.adjust(&proposer, &THRESHOLD);
    votes.adjust(&alice, &10_000);
    votes.adjust(&bob, &10_000);
    votes.adjust(&Address::generate(&env), &79_000); // supply = 100_000
    let h = Harness {
        env,
        gov,
        tl,
        votes,
        proposer,
        guardian,
        alice,
        bob,
    };
    h.roll(1);
    h
}

impl Harness<'_> {
    fn roll(&self, ledgers: u32) {
        let seq = self.env.ledger().sequence();
        self.env.ledger().set_sequence_number(seq + ledgers);
    }

    fn warp(&self, secs: u64) {
        let t = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(t + secs);
    }

    /// A proposal that sets the governor's own quorum to `bps`.
    fn propose_quorum(&self, bps: u32) -> BytesN<32> {
        let (t, f, a) = self.quorum_action(bps);
        self.gov.propose(
            &self.proposer,
            &t,
            &f,
            &a,
            &BytesN::from_array(&self.env, &[7; 32]),
        )
    }

    fn quorum_action(&self, bps: u32) -> (Vec<Address>, Vec<Symbol>, Vec<Vec<Val>>) {
        let e = &self.env;
        (
            vec![e, self.gov.address.clone()],
            vec![e, Symbol::new(e, "set_quorum_bps")],
            vec![e, vec![e, bps.into_val(e)]],
        )
    }

    fn to_active(&self) {
        self.roll(DELAY_LEDGERS + 1);
    }

    fn to_ended(&self) {
        self.roll(PERIOD_LEDGERS);
    }

    fn passed(&self) -> BytesN<32> {
        let id = self.propose_quorum(500);
        self.to_active();
        self.gov.cast_vote(&self.alice, &id, &1);
        self.to_ended();
        id
    }
}

#[test]
fn full_lifecycle_through_timelock() {
    let h = setup();
    let id = h.propose_quorum(500);
    assert_eq!(h.gov.state(&id), ProposalState::Pending);
    assert_eq!(
        h.gov.try_cast_vote(&h.alice, &id, &1),
        Err(Ok(Error::InvalidState))
    );

    h.to_active();
    assert_eq!(h.gov.state(&id), ProposalState::Active);
    assert_eq!(h.gov.cast_vote(&h.alice, &id, &1), 10_000);
    assert_eq!(h.gov.cast_vote(&h.bob, &id, &2), 10_000);
    assert!(h.gov.has_voted(&id, &h.alice));

    h.to_ended();
    assert_eq!(h.gov.state(&id), ProposalState::Succeeded);
    let eta = h.gov.queue(&id);
    assert_eq!(eta, h.env.ledger().timestamp() + TL_DELAY);
    assert_eq!(h.gov.state(&id), ProposalState::Queued);
    assert!(h.tl.try_execute(&id).is_err()); // delay not elapsed

    h.warp(TL_DELAY);
    h.env.set_auths(&[]); // execution needs no signature at all
    h.tl.execute(&id);
    assert_eq!(h.gov.state(&id), ProposalState::Executed);
    assert_eq!(h.gov.quorum_bps(), 500);
}

#[test]
fn tie_is_defeat() {
    let h = setup();
    let id = h.propose_quorum(500);
    h.to_active();
    h.gov.cast_vote(&h.alice, &id, &1);
    h.gov.cast_vote(&h.bob, &id, &0);
    h.to_ended();
    assert_eq!(h.gov.state(&id), ProposalState::Defeated);
    assert_eq!(h.gov.try_queue(&id), Err(Ok(Error::InvalidState)));
}

#[test]
fn quorum_not_met_is_defeat() {
    let h = setup();
    let id = h.propose_quorum(500);
    h.to_active();
    h.gov.cast_vote(&h.proposer, &id, &1); // 1_000 < 4% of 100_000
    h.to_ended();
    assert_eq!(
        h.gov.quorum(&h.gov.get_proposal(&id).unwrap().snapshot),
        4_000
    );
    assert_eq!(h.gov.state(&id), ProposalState::Defeated);
}

#[test]
fn flash_votes_carry_no_weight() {
    let h = setup();
    let id = h.propose_quorum(500);
    h.to_active();
    let whale = Address::generate(&h.env);
    h.votes.adjust(&whale, &1_000_000); // acquired after the snapshot
    assert_eq!(h.gov.cast_vote(&whale, &id, &1), 0);
    h.to_ended();
    assert_eq!(h.gov.state(&id), ProposalState::Defeated);
}

#[test]
fn double_vote_rejected() {
    let h = setup();
    let id = h.propose_quorum(500);
    h.to_active();
    h.gov.cast_vote(&h.alice, &id, &1);
    assert_eq!(
        h.gov.try_cast_vote(&h.alice, &id, &0),
        Err(Ok(Error::AlreadyVoted))
    );
    assert_eq!(
        h.gov.try_cast_vote_by_sig(&h.alice, &id, &1),
        Err(Ok(Error::AlreadyVoted))
    );
}

#[test]
fn vote_after_deadline_rejected() {
    let h = setup();
    let id = h.propose_quorum(500);
    h.to_active();
    h.to_ended();
    assert_eq!(
        h.gov.try_cast_vote(&h.alice, &id, &1),
        Err(Ok(Error::InvalidState))
    );
}

#[test]
fn invalid_support_rejected() {
    let h = setup();
    let id = h.propose_quorum(500);
    h.to_active();
    assert_eq!(
        h.gov.try_cast_vote(&h.alice, &id, &3),
        Err(Ok(Error::InvalidSupport))
    );
}

#[test]
fn cast_vote_by_sig_counts() {
    let h = setup();
    let id = h.propose_quorum(500);
    h.to_active();
    assert_eq!(h.gov.cast_vote_by_sig(&h.bob, &id, &0), 10_000);
    assert_eq!(h.gov.get_proposal(&id).unwrap().against_votes, 10_000);
}

#[test]
fn propose_requires_threshold_and_unique_id() {
    let h = setup();
    let (t, f, a) = h.quorum_action(500);
    let desc = BytesN::from_array(&h.env, &[7; 32]);
    let poor = Address::generate(&h.env);
    assert_eq!(
        h.gov.try_propose(&poor, &t, &f, &a, &desc),
        Err(Ok(Error::BelowProposalThreshold))
    );
    h.gov.propose(&h.proposer, &t, &f, &a, &desc);
    assert_eq!(
        h.gov.try_propose(&h.proposer, &t, &f, &a, &desc),
        Err(Ok(Error::ProposalExists))
    );
    assert_eq!(
        h.gov.try_propose(
            &h.proposer,
            &vec![&h.env],
            &vec![&h.env],
            &vec![&h.env],
            &desc
        ),
        Err(Ok(Error::InvalidProposal))
    );
}

#[test]
fn proposer_cancels_while_pending_only() {
    let h = setup();
    let id = h.propose_quorum(500);
    h.to_active();
    assert_eq!(
        h.gov.try_cancel(&h.proposer, &id),
        Err(Ok(Error::Unauthorized))
    );

    let id2 = h.propose_quorum(600);
    h.gov.cancel(&h.proposer, &id2);
    assert_eq!(h.gov.state(&id2), ProposalState::Canceled);
    assert_eq!(
        h.gov.try_cast_vote(&h.alice, &id2, &1),
        Err(Ok(Error::InvalidState))
    );
}

#[test]
fn anyone_cancels_when_proposer_drops_below_threshold() {
    let h = setup();
    let id = h.passed();
    h.gov.queue(&id);
    let stranger = Address::generate(&h.env);
    assert_eq!(
        h.gov.try_cancel(&stranger, &id),
        Err(Ok(Error::Unauthorized))
    );

    h.votes.adjust(&h.proposer, &-1);
    h.roll(1);
    h.gov.cancel(&stranger, &id);
    assert_eq!(h.gov.state(&id), ProposalState::Canceled);
    assert!(h.tl.get_operation(&id).is_none());
    h.warp(TL_DELAY);
    assert!(h.tl.try_execute(&id).is_err());
}

#[test]
fn queued_proposal_expires() {
    let h = setup();
    let id = h.passed();
    h.gov.queue(&id);
    h.warp(TL_DELAY + TL_GRACE + 1);
    assert_eq!(h.gov.state(&id), ProposalState::Expired);
    assert!(h.tl.try_execute(&id).is_err());
}

#[test]
fn guardian_can_veto_queued_proposal() {
    let h = setup();
    let id = h.passed();
    h.gov.queue(&id);
    h.tl.cancel(&h.guardian, &id);
    h.warp(TL_DELAY);
    assert!(h.tl.try_execute(&id).is_err());
}

#[test]
fn params_only_via_governance_and_within_bounds() {
    let h = setup();
    h.env.set_auths(&[]);
    assert!(matches!(h.gov.try_set_voting_period(&1_000), Err(Err(_))));
    h.env.mock_all_auths();
    assert_eq!(h.gov.try_set_voting_period(&1), Err(Ok(Error::OutOfBounds)));
    assert_eq!(
        h.gov.try_set_voting_delay(&(crate::MAX_VOTING_DELAY + 1)),
        Err(Ok(Error::OutOfBounds))
    );
    assert_eq!(h.gov.try_set_quorum_bps(&50), Err(Ok(Error::OutOfBounds)));
    assert_eq!(
        h.gov.try_set_quorum_bps(&10_001),
        Err(Ok(Error::OutOfBounds))
    );
    assert_eq!(
        h.gov.try_set_proposal_threshold(&-1),
        Err(Ok(Error::OutOfBounds))
    );
    h.gov.set_voting_period(&1_000);
    assert_eq!(h.gov.voting_period(), 1_000);
}

#[test]
fn reinitialize_and_unknown_proposals_are_typed_errors() {
    let h = setup();
    assert_eq!(
        h.gov.try_initialize(
            &h.votes.address,
            &h.tl.address,
            &DELAY_LEDGERS,
            &PERIOD_LEDGERS,
            &QUORUM_BPS,
            &THRESHOLD,
        ),
        Err(Ok(Error::AlreadyInitialized))
    );
    let unknown = BytesN::from_array(&h.env, &[1; 32]);
    assert_eq!(h.gov.try_state(&unknown), Err(Ok(Error::ProposalNotFound)));
}

#[test]
fn uninitialized_governor_reports_not_initialized() {
    let env = Env::default();
    let gov = GovernorClient::new(&env, &env.register_contract(None, Governor));
    assert_eq!(gov.try_voting_delay(), Err(Ok(Error::NotInitialized)));
    assert_eq!(gov.try_get_timelock(), Err(Ok(Error::NotInitialized)));
}
