#![cfg(test)]

use crate::{
    Call, DataKey, Error, Operation, Role, Status, Timelock, TimelockClient, MAX_CHAIN_DEPTH,
};
use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::{Address as _, Ledger},
    vec, Address, BytesN, Env, Vec,
};

const DELAY: u64 = 100;

#[contract]
pub struct Target;

#[contractimpl]
impl Target {
    pub fn poke(env: Env) {
        let n: u32 = env
            .storage()
            .instance()
            .get(&symbol_short!("n"))
            .unwrap_or(0);
        env.storage().instance().set(&symbol_short!("n"), &(n + 1));
    }
    pub fn count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("n"))
            .unwrap_or(0)
    }
}

struct Ctx<'a> {
    env: Env,
    t: TimelockClient<'a>,
    target: Address,
    admin: Address,
}

fn setup<'a>() -> Ctx<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let id = env.register_contract(None, Timelock);
    let t = TimelockClient::new(&env, &id);
    t.initialize(
        &DELAY,
        &vec![&env, admin.clone()],
        &Vec::new(&env),
        &vec![&env, admin.clone()],
    );
    let target = env.register_contract(None, Target);
    Ctx {
        env,
        t,
        target,
        admin,
    }
}

impl<'a> Ctx<'a> {
    fn calls(&self) -> Vec<Call> {
        vec![
            &self.env,
            Call {
                target: self.target.clone(),
                function: symbol_short!("poke"),
                args: Vec::new(&self.env),
            },
        ]
    }
    fn salt(&self, n: u8) -> BytesN<32> {
        BytesN::from_array(&self.env, &[n; 32])
    }
    fn try_sched(&self, pred: &Option<BytesN<32>>, n: u8) -> Result<BytesN<32>, Error> {
        match self
            .t
            .try_schedule(&self.admin, &self.calls(), pred, &self.salt(n), &DELAY)
        {
            Ok(Ok(id)) => Ok(id),
            Err(Ok(e)) => Err(e),
            other => panic!("unexpected {:?}", other),
        }
    }
    fn sched(&self, pred: Option<BytesN<32>>, n: u8) -> BytesN<32> {
        self.try_sched(&pred, n).unwrap()
    }
    fn try_exec(&self, pred: &Option<BytesN<32>>, n: u8) -> Result<(), Error> {
        match self
            .t
            .try_execute(&self.admin, &self.calls(), pred, &self.salt(n))
        {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(e),
            other => panic!("unexpected {:?}", other),
        }
    }
    fn wait(&self) {
        let now = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(now + DELAY);
    }
    fn forge(&self, id: &BytesN<32>, status: Status, pred: Option<BytesN<32>>) {
        self.env.as_contract(&self.t.address, || {
            self.env.storage().persistent().set(
                &DataKey::Op(id.clone()),
                &Operation {
                    status,
                    ready_at: 0,
                    predecessor: match pred {
                        Some(p) => vec![&self.env, p],
                        None => Vec::new(&self.env),
                    },
                },
            );
        });
    }
}

#[test]
fn unknown_predecessor_is_rejected() {
    let c = setup();
    assert_eq!(c.try_sched(&Some(c.salt(9)), 1), Err(Error::ActionNotFound));
}

#[test]
fn cancelled_predecessor_is_rejected_at_schedule() {
    let c = setup();
    let a = c.sched(None, 1);
    c.t.cancel(&c.admin, &a);
    assert_eq!(c.try_sched(&Some(a), 2), Err(Error::PredecessorCancelled));
}

#[test]
fn execute_before_predecessor_is_rejected() {
    let c = setup();
    let a = c.sched(None, 1);
    let b = c.sched(Some(a.clone()), 2);
    c.wait();
    assert_eq!(
        c.try_exec(&Some(a.clone()), 2),
        Err(Error::PredecessorNotExecuted)
    );
    c.try_exec(&None, 1).unwrap();
    c.try_exec(&Some(a), 2).unwrap();
    assert!(c.t.is_operation_done(&b));
}

#[test]
fn cancelling_predecessor_makes_dependent_non_executable() {
    let c = setup();
    let a = c.sched(None, 1);
    c.sched(Some(a.clone()), 2);
    c.t.cancel(&c.admin, &a);
    c.wait();
    assert_eq!(c.try_exec(&Some(a), 2), Err(Error::PredecessorCancelled));
}

#[test]
fn cancelled_ancestor_blocks_a_done_looking_chain() {
    let c = setup();
    let a = c.sched(None, 1);
    let b = c.sched(Some(a.clone()), 2);
    c.sched(Some(b.clone()), 3);
    c.t.cancel(&c.admin, &a);
    c.wait();
    assert_eq!(c.try_exec(&Some(b), 3), Err(Error::PredecessorCancelled));
}

#[test]
fn self_referencing_predecessor_is_rejected() {
    let c = setup();
    // An id commits to its predecessor, so "itself" can only be named as
    // an id that is not stored yet.
    let own = c.t.hash_operation(&c.calls(), &Some(c.salt(1)), &c.salt(1));
    assert_eq!(c.try_sched(&Some(own), 1), Err(Error::ActionNotFound));
}

#[test]
fn cycle_through_existing_edge_is_rejected() {
    let c = setup();
    // P already names the (not yet stored) id of the op being scheduled.
    let p = c.salt(7);
    let a = c.t.hash_operation(&c.calls(), &Some(p.clone()), &c.salt(1));
    c.forge(&p, Status::Pending, Some(a));
    assert_eq!(c.try_sched(&Some(p), 1), Err(Error::PredecessorCycle));
}

#[test]
fn three_node_cycle_is_rejected_not_walked_forever() {
    let c = setup();
    let (a, b, d) = (c.salt(11), c.salt(12), c.salt(13));
    c.forge(&a, Status::Pending, Some(b.clone()));
    c.forge(&b, Status::Pending, Some(d.clone()));
    c.forge(&d, Status::Pending, Some(a.clone()));
    assert_eq!(
        c.try_sched(&Some(a), 1),
        Err(Error::PredecessorChainTooDeep)
    );
}

#[test]
fn long_valid_chain_executes_in_order() {
    let c = setup();
    let mut ids: Vec<BytesN<32>> = Vec::new(&c.env);
    let mut pred = None;
    for n in 0..MAX_CHAIN_DEPTH {
        let id = c.sched(pred.clone(), n as u8 + 1);
        ids.push_back(id.clone());
        pred = Some(id);
    }
    c.wait();
    for n in 0..MAX_CHAIN_DEPTH {
        let p = if n == 0 { None } else { ids.get(n - 1) };
        c.try_exec(&p, n as u8 + 1).unwrap();
    }
    assert_eq!(
        TargetClient::new(&c.env, &c.target).count(),
        MAX_CHAIN_DEPTH
    );
}

#[test]
fn chain_beyond_max_depth_is_rejected() {
    let c = setup();
    let mut pred = None;
    for n in 0..=MAX_CHAIN_DEPTH {
        pred = Some(c.sched(pred, n as u8 + 1));
    }
    assert_eq!(c.try_sched(&pred, 100), Err(Error::PredecessorChainTooDeep));
}

#[test]
fn schedule_with_predecessor_stays_within_budget() {
    let c = setup();
    let a = c.sched(None, 1);
    let mut b = a.clone();
    for n in 2..=MAX_CHAIN_DEPTH as u8 {
        b = c.sched(Some(b), n);
    }
    c.env.budget().reset_default();
    c.sched(Some(b), 50);
    assert!(c.env.budget().cpu_instruction_cost() < 25_000_000);
}

#[test]
fn roles_gate_schedule_and_cancel() {
    let c = setup();
    let stranger = Address::generate(&c.env);
    assert!(!c.t.has_role(&Role::Proposer, &stranger));
    let r =
        c.t.try_schedule(&stranger, &c.calls(), &None, &c.salt(1), &DELAY);
    assert_eq!(r, Err(Ok(Error::Unauthorized)));
}
