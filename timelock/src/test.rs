#![cfg(test)]

extern crate std;

use crate::{Call, ProposalStatus, Timelock, TimelockClient, MAX_PAGE, MAX_PENDING};
use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger},
    vec, Address, Env, IntoVal, Symbol,
};

#[contract]
pub struct Target;

#[contractimpl]
impl Target {
    pub fn ping(_env: Env, v: u32) -> u32 {
        v + 1
    }
}

const DELAY: u64 = 100;

struct H<'a> {
    env: Env,
    tl: TimelockClient<'a>,
    target: Address,
    canceller: Address,
    guardian: Address,
}

fn setup<'a>() -> H<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let tl = TimelockClient::new(&env, &env.register_contract(None, Timelock));
    let (p, e, c, g) = (
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    );
    tl.initialize(&p, &e, &c, &DELAY, &Some(g.clone()));
    let target = env.register_contract(None, Target);
    H {
        env,
        tl,
        target,
        canceller: c,
        guardian: g,
    }
}

impl H<'_> {
    fn call(&self, v: u32) -> Call {
        Call {
            target: self.target.clone(),
            func: Symbol::new(&self.env, "ping"),
            args: vec![&self.env, v.into_val(&self.env)],
        }
    }
    fn propose(&self, pred: Option<u64>) -> u64 {
        self.tl.propose(&self.call(1), &pred)
    }
    fn advance(&self, secs: u64) {
        let t = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(t + secs);
    }
}

#[test]
fn proposal_is_queryable_through_its_lifecycle() {
    let h = setup();
    assert!(h.tl.get_proposal(&0).is_none());
    let id = h.propose(None);
    let p = h.tl.get_proposal(&id).unwrap();
    assert_eq!(p.status, ProposalStatus::Pending);
    assert_eq!(p.execute_after, DELAY);
    assert_eq!(h.tl.next_executable_at(&id), Some(DELAY));
    assert!(!h.tl.is_executable(&id));

    h.advance(DELAY);
    assert!(h.tl.is_executable(&id));
    h.tl.execute(&id);
    assert!(h.tl.is_executed(&id));
    assert!(!h.tl.is_executable(&id));
    assert_eq!(h.tl.next_executable_at(&id), None);
    assert_eq!(h.tl.pending_count(), 0);
}

#[test]
fn cancelled_proposals_report_cancelled_and_leave_the_pending_list() {
    let h = setup();
    let a = h.propose(None);
    let b = h.propose(None);
    h.tl.cancel(&h.canceller, &a);
    assert!(h.tl.is_cancelled(&a));
    assert!(!h.tl.is_cancelled(&b));
    assert_eq!(h.tl.get_pending(&0, &10), vec![&h.env, b]);
    h.tl.cancel(&h.guardian, &b);
    assert!(h.tl.is_cancelled(&b));
}

#[test]
fn unknown_ids_are_not_executable_executed_or_cancelled() {
    let h = setup();
    assert!(!h.tl.is_executable(&9));
    assert!(!h.tl.is_executed(&9));
    assert!(!h.tl.is_cancelled(&9));
    assert_eq!(h.tl.next_executable_at(&9), None);
    assert_eq!(h.tl.predecessor(&9), None);
}

#[test]
fn predecessor_gates_executability() {
    let h = setup();
    let first = h.propose(None);
    let second = h.propose(Some(first));
    assert_eq!(h.tl.predecessor(&second), Some(first));
    h.advance(DELAY);
    assert!(h.tl.is_executable(&first));
    assert!(!h.tl.is_executable(&second));
    assert!(h.tl.try_execute(&second).is_err());
    h.tl.execute(&first);
    assert!(h.tl.is_executable(&second));
    h.tl.execute(&second);
}

#[test]
fn execute_before_the_delay_or_twice_fails() {
    let h = setup();
    let id = h.propose(None);
    assert!(h.tl.try_execute(&id).is_err());
    h.advance(DELAY);
    h.tl.execute(&id);
    assert!(h.tl.try_execute(&id).is_err());
}

#[test]
fn roles_are_exposed() {
    let h = setup();
    assert_eq!(h.tl.delay(), DELAY);
    assert_eq!(h.tl.canceller(), h.canceller);
    assert_eq!(h.tl.guardian(), Some(h.guardian.clone()));
    h.tl.proposer();
    h.tl.executor();
}

#[test]
fn pagination_first_last_partial_and_out_of_range() {
    let h = setup();
    for _ in 0..25 {
        h.propose(None);
    }
    assert_eq!(h.tl.get_pending(&0, &10).len(), 10);
    assert_eq!(h.tl.get_pending(&0, &10).get(0), Some(0));
    assert_eq!(h.tl.get_pending(&20, &10).len(), 5);
    assert_eq!(h.tl.get_pending(&25, &10).len(), 0);
    assert_eq!(h.tl.get_pending(&u32::MAX, &10).len(), 0);
}

#[test]
fn pagination_rejects_zero_and_oversized_limits() {
    let h = setup();
    assert!(h.tl.try_get_pending(&0, &0).is_err());
    assert!(h.tl.try_get_pending(&0, &(MAX_PAGE + 1)).is_err());
    assert_eq!(h.tl.get_pending(&0, &MAX_PAGE).len(), 0);
}

#[test]
fn max_page_over_a_full_queue_stays_within_budget() {
    let h = setup();
    for _ in 0..MAX_PENDING {
        h.propose(None);
    }
    assert!(h.tl.try_propose(&h.call(1), &None).is_err());
    h.env.budget().reset_default();
    let page = h.tl.get_pending(&(MAX_PENDING - MAX_PAGE), &MAX_PAGE);
    assert_eq!(page.len(), MAX_PAGE);
    assert!(h.env.budget().cpu_instruction_cost() < 5_000_000);
}

#[test]
fn queries_do_not_mutate_storage_or_extend_ttls() {
    let h = setup();
    let id = h.propose(None);
    h.env.ledger().with_mut(|l| l.sequence_number += 1_000);
    let snapshot = |env: &Env| std::format!("{:?}", env.to_ledger_snapshot().ledger_entries);
    let before = snapshot(&h.env);
    h.tl.get_proposal(&id);
    h.tl.is_executable(&id);
    h.tl.is_executed(&id);
    h.tl.is_cancelled(&id);
    h.tl.next_executable_at(&id);
    h.tl.predecessor(&id);
    h.tl.get_pending(&0, &10);
    assert_eq!(snapshot(&h.env), before);
    assert_eq!(
        h.tl.get_proposal(&id).unwrap().status,
        ProposalStatus::Pending
    );
}

#[test]
fn cancel_requires_canceller_or_guardian() {
    let h = setup();
    let id = h.propose(None);
    let stranger = Address::generate(&h.env);
    assert!(h.tl.try_cancel(&stranger, &id).is_err());
}
