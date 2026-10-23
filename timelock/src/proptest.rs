#![cfg(test)]

//! Property-based safety suite for the timelock (issue #269).
//!
//! Each generated case is a random proposal graph (predecessor edges
//! including chains, diamonds, self-edges and forward edges that would
//! close cycles), random delays, cancellations, a mid-run `set_delay`, a
//! random executor role, and a random walk through time. The case is
//! driven against the real contract through `Env` and checked against a
//! small model after every call:
//!
//! - `execute` succeeds exactly when the operation is pending, not
//!   cancelled, inside `[ready_at, ready_at + grace]`, and every
//!   predecessor is done — so it reverts before its delay and while any
//!   predecessor is unexecuted;
//! - an operation with a cancelled predecessor is never executed;
//! - an address without the role never schedules, cancels, or (when an
//!   executor is set) executes;
//! - a `set_delay` never moves an already-scheduled `ready_at`;
//! - self-edges and edges to not-yet-scheduled operations (the only way
//!   to build a cycle) are rejected at schedule time with
//!   `InvalidPredecessor`. This pins the implementation's choice:
//!   cycles are impossible to schedule, rather than unexecutable.
//!
//! Runs are deterministically seeded. On failure proptest shrinks to a
//! minimal case and the panic message includes the seed; rerun with
//! `TIMELOCK_PROPTEST_SEED=<seed>` to reproduce. `BrokenTimelock` below
//! ignores delays and predecessors, and `properties_catch_a_broken_timelock`
//! checks the same properties reject it.

extern crate std;

use std::{format, string::String, vec::Vec as StdVec};

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, RngSeed, TestCaseError, TestRunner};
use soroban_sdk::{
    contract, contractimpl, panic_with_error,
    testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
    vec, Address, BytesN, Env, IntoVal, Symbol, Val, Vec,
};

use crate::error::Error;
use crate::{Operation, Timelock, TimelockClient};

const MIN_DELAY: u64 = 3_600;
const GRACE: u64 = 3 * 86_400;
const MAX_OPS: usize = 6;
const DEFAULT_SEED: u64 = 0x7153_2690;
const CASES: u32 = 48;

/// Stand-in for an admin-gated protocol contract.
#[contract]
struct Target;

#[contractimpl]
impl Target {
    pub fn init(env: Env, admin: Address) {
        env.storage().instance().set(&0u32, &admin);
    }
    pub fn set_value(env: Env, value: u32) {
        let admin: Address = env.storage().instance().get(&0u32).unwrap();
        admin.require_auth();
        env.storage().instance().set(&1u32, &value);
    }
}

#[derive(Clone, Debug)]
struct OpSpec {
    delay: u64,
    /// Indices into the case's ops; `>= own index` is a self or forward
    /// edge and must be rejected.
    preds: StdVec<usize>,
    cancel: bool,
}

#[derive(Clone, Debug)]
struct Case {
    ops: StdVec<OpSpec>,
    guardian_is_proposer: bool,
    restricted_executor: bool,
    /// New min delay applied by a self-call once ready (shorter or longer).
    new_min_delay: u64,
    /// Time jumps between execution rounds.
    steps: StdVec<u64>,
    /// Order operations are attempted in each round.
    order: StdVec<usize>,
}

fn op_spec(n: usize) -> impl Strategy<Value = OpSpec> {
    (
        MIN_DELAY..MIN_DELAY + 2 * 86_400,
        prop::collection::vec(0..n, 0..3),
        prop::bool::weighted(0.2),
    )
        .prop_map(|(delay, mut preds, cancel)| {
            preds.sort_unstable();
            preds.dedup();
            OpSpec {
                delay,
                preds,
                cancel,
            }
        })
}

fn case() -> impl Strategy<Value = Case> {
    (1..=MAX_OPS).prop_flat_map(|n| {
        (
            prop::collection::vec(op_spec(n), n),
            any::<bool>(),
            any::<bool>(),
            1..4 * MIN_DELAY,
            prop::collection::vec(0..2 * 86_400u64, 1..8),
            Just((0..n).collect::<StdVec<_>>()).prop_shuffle(),
        )
            .prop_map(
                |(ops, guardian_is_proposer, restricted_executor, new_min_delay, steps, order)| {
                    Case {
                        ops,
                        guardian_is_proposer,
                        restricted_executor,
                        new_min_delay,
                        steps,
                        order,
                    }
                },
            )
    })
}

/// Model of one scheduled operation.
#[derive(Clone)]
struct Model {
    ready_at: u64,
    preds: StdVec<usize>,
    cancelled: bool,
    done: bool,
}

fn op_id(env: &Env, n: usize) -> BytesN<32> {
    BytesN::from_array(env, &[n as u8 + 1; 32])
}

fn check(cond: bool, msg: impl FnOnce() -> String) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(msg())
    }
}

struct World<'a> {
    env: Env,
    tl: TimelockClient<'a>,
    target: Address,
    proposer: Address,
    guardian: Address,
    executor: Option<Address>,
    outsider: Address,
}

impl World<'_> {
    fn new(register: fn(&Env) -> Address, case: &Case) -> Self {
        let env = Env::default();
        env.mock_all_auths();
        let tl = TimelockClient::new(&env, &register(&env));
        let proposer = Address::generate(&env);
        let guardian = if case.guardian_is_proposer {
            proposer.clone()
        } else {
            Address::generate(&env)
        };
        tl.initialize(&proposer, &guardian, &MIN_DELAY, &GRACE);
        let target = env.register_contract(None, Target);
        TargetClient::new(&env, &target).init(&tl.address);
        let outsider = Address::generate(&env);
        World {
            env,
            tl,
            target,
            proposer,
            guardian,
            executor: None,
            outsider,
        }
    }

    fn now(&self) -> u64 {
        self.env.ledger().timestamp()
    }

    fn warp(&self, dt: u64) {
        self.env.ledger().set_timestamp(self.now() + dt);
    }

    fn call(&self, value: u32) -> (Vec<Address>, Vec<Symbol>, Vec<Vec<Val>>) {
        (
            vec![&self.env, self.target.clone()],
            vec![&self.env, Symbol::new(&self.env, "set_value")],
            vec![&self.env, vec![&self.env, value.into_val(&self.env)]],
        )
    }

    fn self_call(&self, func: &str, args: Vec<Val>) -> (Vec<Address>, Vec<Symbol>, Vec<Vec<Val>>) {
        (
            vec![&self.env, self.tl.address.clone()],
            vec![&self.env, Symbol::new(&self.env, func)],
            vec![&self.env, args],
        )
    }

    /// Authorizes only `who` for the next call to `func`.
    fn only(&self, who: &Address, func: &str, args: Vec<Val>) {
        self.env.mock_auths(&[MockAuth {
            address: who,
            invoke: &MockAuthInvoke {
                contract: &self.tl.address,
                fn_name: func,
                args,
                sub_invokes: &[],
            },
        }]);
    }

    /// Schedules and immediately runs a self-call (used to set up roles).
    fn run_self(&mut self, id: BytesN<32>, func: &str, args: Vec<Val>) {
        let (t, f, a) = self.self_call(func, args);
        self.tl.schedule(&id, &t, &f, &a, &MIN_DELAY);
        self.warp(MIN_DELAY);
        self.tl.execute(&id);
    }
}

fn ready_ats(w: &World, model: &[Option<Model>]) -> Result<(), String> {
    for (i, m) in model.iter().enumerate() {
        let Some(m) = m else { continue };
        if m.cancelled {
            continue;
        }
        let op: Option<Operation> = w.tl.get_operation(&op_id(&w.env, i));
        let eta = op.map(|o| o.ready_at);
        check(eta == Some(m.ready_at), || {
            format!("op {i}: ready_at moved from {} to {eta:?}", m.ready_at)
        })?;
    }
    Ok(())
}

/// Drives `case` against the contract `register` installs and returns the
/// first property violation.
fn run_case(register: fn(&Env) -> Address, case: &Case) -> Result<(), String> {
    let mut w = World::new(register, case);
    let n = case.ops.len();

    if case.restricted_executor {
        let executor = Address::generate(&w.env);
        w.run_self(
            BytesN::from_array(&w.env, &[0xE0; 32]),
            "set_exec",
            vec![&w.env, executor.into_val(&w.env)],
        );
        w.executor = Some(executor);
    }

    // Unauthorized scheduling never succeeds.
    let probe = BytesN::from_array(&w.env, &[0xA0; 32]);
    let (t, f, a) = w.call(0);
    w.only(
        &w.outsider,
        "schedule",
        (probe.clone(), t.clone(), f.clone(), a.clone(), MIN_DELAY).into_val(&w.env),
    );
    let outsider_scheduled = w.tl.try_schedule(&probe, &t, &f, &a, &MIN_DELAY).is_ok();
    w.env.mock_all_auths();
    check(!outsider_scheduled, || "outsider scheduled".into())?;

    // Schedule the graph.
    let mut model: StdVec<Option<Model>> = (0..n).map(|_| None).collect();
    for (i, spec) in case.ops.iter().enumerate() {
        let (t, f, a) = w.call(i as u32 + 1);
        let id = op_id(&w.env, i);
        let mut preds = Vec::new(&w.env);
        for p in &spec.preds {
            preds.push_back(op_id(&w.env, *p));
        }
        let expect_ok = spec.preds.iter().all(|p| *p < i && model[*p].is_some());
        let res =
            w.tl.try_schedule_after(&id, &t, &f, &a, &spec.delay, &preds);
        match (expect_ok, res) {
            (true, Ok(Ok(ready_at))) => {
                check(ready_at == w.now() + spec.delay, || {
                    format!("op {i}: ready_at {ready_at} != now + delay")
                })?;
                model[i] = Some(Model {
                    ready_at,
                    preds: spec.preds.clone(),
                    cancelled: false,
                    done: false,
                });
            }
            (false, Err(Ok(e))) => check(e == Error::InvalidPredecessor.into(), || {
                format!("op {i}: self/forward edge rejected with {e:?}")
            })?,
            (true, res) => return Err(format!("op {i}: valid schedule failed: {res:?}")),
            (false, res) => {
                return Err(format!(
                    "op {i}: self/forward edge {:?} accepted: {res:?}",
                    spec.preds
                ))
            }
        }
    }

    // Cancellations: never by an outsider, always by proposer/guardian.
    for (i, spec) in case.ops.iter().enumerate() {
        if !spec.cancel || model[i].is_none() {
            continue;
        }
        let id = op_id(&w.env, i);
        let outsider_cancelled = w.tl.try_cancel(&w.outsider, &id).is_ok();
        check(!outsider_cancelled, || format!("outsider cancelled op {i}"))?;
        let by = if i % 2 == 0 { &w.proposer } else { &w.guardian };
        check(w.tl.try_cancel(by, &id).is_ok(), || {
            format!("role holder couldn't cancel op {i}")
        })?;
        model[i].as_mut().unwrap().cancelled = true;
    }

    // A min-delay change, executed whenever it becomes ready below.
    let delay_op = BytesN::from_array(&w.env, &[0xD0; 32]);
    let (t, f, a) = w.self_call(
        "set_delay",
        vec![&w.env, case.new_min_delay.into_val(&w.env)],
    );
    w.tl.schedule(&delay_op, &t, &f, &a, &MIN_DELAY);
    let delay_ready = w.now() + MIN_DELAY;

    for dt in &case.steps {
        w.warp(*dt);
        let now = w.now();
        if now >= delay_ready && now <= delay_ready + GRACE && !w.tl.is_done(&delay_op) {
            if let Some(ex) = &w.executor {
                w.only(ex, "execute", (delay_op.clone(),).into_val(&w.env));
            }
            let ran = w.tl.try_execute(&delay_op).is_ok();
            w.env.mock_all_auths();
            check(ran, || "set_delay op failed to execute".into())?;
            ready_ats(&w, &model)?;
        }

        for &i in &case.order {
            let Some(m) = model[i].clone() else { continue };
            if m.done {
                continue;
            }
            let id = op_id(&w.env, i);
            let preds_done = m
                .preds
                .iter()
                .all(|p| model[*p].as_ref().is_some_and(|pm| pm.done));
            let expect_ok =
                !m.cancelled && now >= m.ready_at && now <= m.ready_at + GRACE && preds_done;

            if let Some(ex) = w.executor.clone() {
                w.only(&w.outsider, "execute", (id.clone(),).into_val(&w.env));
                let outsider_ran = w.tl.try_execute(&id).is_ok();
                check(!outsider_ran, || format!("outsider executed op {i}"))?;
                w.only(&ex, "execute", (id.clone(),).into_val(&w.env));
            }
            let ran = w.tl.try_execute(&id).is_ok();
            w.env.mock_all_auths();
            check(ran == expect_ok, || {
                format!(
                    "op {i} at t={now}: execute ok={ran}, expected {expect_ok} \
                     (ready_at={}, cancelled={}, preds_done={preds_done})",
                    m.ready_at, m.cancelled
                )
            })?;
            if ran {
                model[i].as_mut().unwrap().done = true;
            }
        }
    }

    // Cancelled predecessors permanently block their dependents.
    for (i, m) in model.iter().enumerate() {
        let Some(m) = m else { continue };
        let blocked = m
            .preds
            .iter()
            .any(|p| model[*p].as_ref().is_some_and(|pm| pm.cancelled));
        let done = w.tl.is_done(&op_id(&w.env, i));
        check(!(blocked && done), || {
            format!("op {i} executed despite a cancelled predecessor")
        })?;
        check(done == m.done, || {
            format!("op {i}: is_done disagrees with model")
        })?;
    }
    ready_ats(&w, &model)
}

fn seed() -> u64 {
    std::env::var("TIMELOCK_PROPTEST_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SEED)
}

fn runner(seed: u64) -> TestRunner {
    TestRunner::new(Config {
        cases: CASES,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    })
}

fn register_real(env: &Env) -> Address {
    env.register_contract(None, Timelock)
}

#[test]
fn timelock_safety_properties_hold() {
    let seed = seed();
    let result = runner(seed).run(&case(), |c| {
        run_case(register_real, &c).map_err(TestCaseError::fail)
    });
    if let Err(e) = result {
        panic!("{e}\nreproduce with TIMELOCK_PROPTEST_SEED={seed}");
    }
}

/// Generated graphs must actually exercise the properties: enough valid
/// predecessor edges, rejected edges, cancellations and executions.
#[test]
fn generator_exercises_the_interesting_paths() {
    let mut runner = runner(seed());
    let (mut valid_edges, mut bad_edges, mut cancels) = (0, 0, 0);
    for _ in 0..CASES {
        let c = case().new_tree(&mut runner).unwrap().current();
        for (i, op) in c.ops.iter().enumerate() {
            for p in &op.preds {
                if *p < i {
                    valid_edges += 1;
                } else {
                    bad_edges += 1;
                }
            }
            cancels += op.cancel as u32;
        }
    }
    assert!(valid_edges >= CASES, "valid edges: {valid_edges}");
    assert!(bad_edges >= CASES / 2, "rejected edges: {bad_edges}");
    assert!(cancels >= CASES / 4, "cancels: {cancels}");
}

// ─── self-check: a deliberately broken timelock ─────────────────────────────

/// Same interface, but `execute` ignores delays, grace and predecessors,
/// and `schedule_after` accepts any edge.
#[contract]
struct BrokenTimelock;

#[contractimpl]
impl BrokenTimelock {
    pub fn initialize(env: Env, proposer: Address, guardian: Address, min_delay: u64, _g: u64) {
        env.storage().instance().set(&0u32, &proposer);
        env.storage().instance().set(&1u32, &guardian);
        env.storage().instance().set(&2u32, &min_delay);
    }

    pub fn schedule(
        env: Env,
        op_id: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
    ) -> u64 {
        Self::schedule_after(
            env.clone(),
            op_id,
            targets,
            fns,
            args,
            delay,
            Vec::new(&env),
        )
    }

    pub fn schedule_after(
        env: Env,
        op_id: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
        predecessors: Vec<BytesN<32>>,
    ) -> u64 {
        let proposer: Address = env.storage().instance().get(&0u32).unwrap();
        proposer.require_auth();
        let ready_at = env.ledger().timestamp() + delay;
        let op = Operation {
            targets,
            fns,
            args,
            ready_at,
            done: false,
            predecessors,
        };
        env.storage().persistent().set(&op_id, &op);
        ready_at
    }

    pub fn execute(env: Env, op_id: BytesN<32>) {
        let mut op: Operation = env
            .storage()
            .persistent()
            .get(&op_id)
            .unwrap_or_else(|| panic_with_error!(&env, Error::OperationNotFound));
        if op.done {
            panic_with_error!(&env, Error::AlreadyDone);
        }
        op.done = true;
        env.storage().persistent().set(&op_id, &op);
        let this = env.current_contract_address();
        for i in 0..op.targets.len() {
            let target = op.targets.get(i).unwrap();
            if target != this {
                env.invoke_contract::<Val>(
                    &target,
                    &op.fns.get(i).unwrap(),
                    op.args.get(i).unwrap(),
                );
            }
        }
    }

    pub fn cancel(env: Env, caller: Address, op_id: BytesN<32>) {
        caller.require_auth();
        let proposer: Address = env.storage().instance().get(&0u32).unwrap();
        let guardian: Address = env.storage().instance().get(&1u32).unwrap();
        if caller != proposer && caller != guardian {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().persistent().remove(&op_id);
    }

    pub fn is_done(env: Env, op_id: BytesN<32>) -> bool {
        Self::get_operation(env, op_id).is_some_and(|op| op.done)
    }

    pub fn get_operation(env: Env, op_id: BytesN<32>) -> Option<Operation> {
        env.storage().persistent().get(&op_id)
    }
}

fn register_broken(env: &Env) -> Address {
    env.register_contract(None, BrokenTimelock)
}

#[test]
fn properties_catch_a_broken_timelock() {
    let result = runner(seed()).run(&case(), |c| {
        run_case(register_broken, &c).map_err(TestCaseError::fail)
    });
    assert!(
        result.is_err(),
        "properties failed to reject BrokenTimelock"
    );
}
