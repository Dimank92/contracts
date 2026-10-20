#![cfg(test)]

use crate::{Role, Timelock, TimelockClient};
use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
    vec, Address, BytesN, Env, IntoVal, Symbol, Val, Vec,
};

const DELAY: u64 = 86_400;
const GRACE: u64 = 7 * 86_400;

/// Stand-in for any admin-gated protocol contract.
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
    pub fn value(env: Env) -> u32 {
        env.storage().instance().get(&1u32).unwrap_or(0)
    }
}

struct Harness<'a> {
    env: Env,
    client: TimelockClient<'a>,
    target: TargetClient<'a>,
    proposer: Address,
    executor: Address,
    canceller: Address,
    /// Holds none of the three roles.
    outsider: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let proposer = Address::generate(&env);
    let executor = Address::generate(&env);
    let canceller = Address::generate(&env);
    let outsider = Address::generate(&env);
    let client = TimelockClient::new(&env, &env.register_contract(None, Timelock));
    client.initialize(
        &DELAY,
        &GRACE,
        &vec![&env, proposer.clone()],
        &vec![&env, executor.clone()],
        &vec![&env, canceller.clone()],
    );
    let target = TargetClient::new(&env, &env.register_contract(None, Target));
    target.init(&client.address);
    Harness {
        env,
        client,
        target,
        proposer,
        executor,
        canceller,
        outsider,
    }
}

impl Harness<'_> {
    fn salt(&self, n: u8) -> BytesN<32> {
        BytesN::from_array(&self.env, &[n; 32])
    }

    fn targets(&self) -> Vec<Address> {
        vec![&self.env, self.target.address.clone()]
    }

    fn fns(&self) -> Vec<Symbol> {
        vec![&self.env, Symbol::new(&self.env, "set_value")]
    }

    fn args(&self, value: u32) -> Vec<Vec<Val>> {
        vec![&self.env, vec![&self.env, value.into_val(&self.env)]]
    }

    fn schedule_set_value(&self, n: u8, value: u32) -> BytesN<32> {
        self.client.schedule(
            &self.proposer,
            &self.targets(),
            &self.fns(),
            &self.args(value),
            &self.salt(n),
            &DELAY,
        )
    }

    fn schedule_set_value_after(&self, n: u8, value: u32, predecessor: &BytesN<32>) -> BytesN<32> {
        self.client.schedule_after(
            &self.proposer,
            &self.targets(),
            &self.fns(),
            &self.args(value),
            predecessor,
            &self.salt(n),
        )
    }

    /// Schedules a call whose target is the timelock itself.
    fn schedule_self(&self, n: u8, func: &str, args: Vec<Val>) -> BytesN<32> {
        self.client.schedule(
            &self.proposer,
            &vec![&self.env, self.client.address.clone()],
            &vec![&self.env, Symbol::new(&self.env, func)],
            &vec![&self.env, args],
            &self.salt(n),
            &DELAY,
        )
    }

    fn warp(&self, dt: u64) {
        let t = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(t + dt);
    }
}

// ── initialization ───────────────────────────────────────────────────────────

#[test]
fn initialize_is_one_shot() {
    let h = setup();
    let e = &h.env;
    assert!(h
        .client
        .try_initialize(
            &DELAY,
            &GRACE,
            &vec![e, h.proposer.clone()],
            &vec![e, h.executor.clone()],
            &vec![e, h.canceller.clone()],
        )
        .is_err());
}

#[test]
fn initialize_refuses_a_delay_below_the_documented_floor() {
    let e = Env::default();
    e.mock_all_auths();
    let client = TimelockClient::new(&e, &e.register_contract(None, Timelock));
    let a = Address::generate(&e);
    assert!(client
        .try_initialize(&1, &GRACE, &vec![&e, a.clone()], &vec![&e], &vec![&e])
        .is_err());
}

#[test]
fn an_empty_executor_set_means_anyone_may_execute() {
    let e = Env::default();
    e.mock_all_auths();
    let client = TimelockClient::new(&e, &e.register_contract(None, Timelock));
    let a = Address::generate(&e);
    client.initialize(&DELAY, &GRACE, &vec![&e, a.clone()], &vec![&e], &vec![&e]);
    assert!(client.is_open_executor());
    assert!(client.has_role(&Role::Proposer, &a));
}

// ── role gating ──────────────────────────────────────────────────────────────

#[test]
fn only_a_proposer_may_schedule() {
    let h = setup();
    assert!(h
        .client
        .try_schedule(
            &h.outsider,
            &h.targets(),
            &h.fns(),
            &h.args(1),
            &h.salt(1),
            &DELAY,
        )
        .is_err());
    assert!(h
        .client
        .try_schedule(
            &h.canceller,
            &h.targets(),
            &h.fns(),
            &h.args(1),
            &h.salt(1),
            &DELAY,
        )
        .is_err());
}

#[test]
fn only_an_executor_may_execute() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    h.warp(DELAY);
    assert!(h.client.try_execute(&h.outsider, &id).is_err());
    // The operation is still pending: the refused attempt changed nothing.
    assert!(h.client.is_operation_pending(&id));
}

#[test]
fn only_a_canceller_may_cancel() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    assert!(h.client.try_cancel(&h.outsider, &id).is_err());
    assert!(h.client.try_cancel(&h.proposer, &id).is_err());
    assert!(h.client.is_operation_pending(&id));
    h.client.cancel(&h.canceller, &id);
    assert!(!h.client.is_operation(&id));
}

// ── the delay ────────────────────────────────────────────────────────────────

#[test]
fn a_delay_shorter_than_min_delay_is_refused() {
    let h = setup();
    assert!(h
        .client
        .try_schedule(
            &h.proposer,
            &h.targets(),
            &h.fns(),
            &h.args(1),
            &h.salt(1),
            &(DELAY - 1),
        )
        .is_err());
}

#[test]
fn execution_is_refused_just_before_the_delay_and_allowed_exactly_at_it() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    assert_eq!(h.client.get_timestamp(&id), DELAY);

    h.warp(DELAY - 1);
    assert!(!h.client.is_operation_ready(&id));
    assert!(h.client.try_execute(&h.executor, &id).is_err());

    h.warp(1);
    assert!(h.client.is_operation_ready(&id));
    h.client.execute(&h.executor, &id);
    assert_eq!(h.target.value(), 42);
}

#[test]
fn an_operation_expires_after_its_grace_period() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    h.warp(DELAY + GRACE + 1);
    assert!(h.client.try_execute(&h.executor, &id).is_err());
    assert_eq!(h.target.value(), 0);
}

// ── lifecycle ────────────────────────────────────────────────────────────────

#[test]
fn an_operation_cannot_execute_twice() {
    let h = setup();
    h.schedule_set_value(1, 42);
    h.warp(DELAY);
    h.client.execute(&h.executor, &h.salt(1));
    assert!(h.client.is_operation_done(&h.salt(1)));
    assert!(h.client.try_execute(&h.executor, &h.salt(1)).is_err());
}

#[test]
fn cancelling_before_execution_stops_it() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    h.client.cancel(&h.canceller, &id);
    h.warp(DELAY);
    assert!(h.client.try_execute(&h.executor, &id).is_err());
    assert_eq!(h.target.value(), 0);
}

#[test]
fn executing_after_a_cancel_is_refused() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    h.client.cancel(&h.canceller, &id);
    assert!(h.client.try_execute(&h.executor, &id).is_err());
}

#[test]
fn cancelling_an_already_executed_operation_is_refused() {
    let h = setup();
    h.schedule_set_value(1, 42);
    h.warp(DELAY);
    h.client.execute(&h.executor, &h.salt(1));
    assert!(h.client.try_cancel(&h.canceller, &h.salt(1)).is_err());
}

#[test]
fn cancelling_an_operation_that_never_existed_is_refused() {
    let h = setup();
    assert!(h.client.try_cancel(&h.canceller, &h.salt(9)).is_err());
}

#[test]
fn scheduling_the_same_batch_twice_is_refused() {
    let h = setup();
    h.schedule_set_value(1, 42);
    assert!(h
        .client
        .try_schedule(
            &h.proposer,
            &h.targets(),
            &h.fns(),
            &h.args(42),
            &h.salt(1),
            &DELAY,
        )
        .is_err());
}

#[test]
fn an_empty_batch_is_refused() {
    let h = setup();
    let e = &h.env;
    assert!(h
        .client
        .try_schedule(
            &h.proposer,
            &vec![e],
            &vec![e],
            &vec![e],
            &h.salt(1),
            &DELAY,
        )
        .is_err());
}

// ── predecessors ─────────────────────────────────────────────────────────────

#[test]
fn a_dependant_waits_for_its_predecessor() {
    let h = setup();
    let first = h.schedule_set_value(1, 11);
    let second = h.schedule_set_value_after(2, 22, &first);
    h.warp(DELAY);

    // The dependant is ready in time, but its predecessor has not run.
    assert!(h.client.is_operation_ready(&second));
    assert!(h.client.try_execute(&h.executor, &second).is_err());
    assert_eq!(h.target.value(), 0);

    h.client.execute(&h.executor, &first);
    h.client.execute(&h.executor, &second);
    assert_eq!(h.target.value(), 22);
}

#[test]
fn cancelling_a_predecessor_strands_its_dependant() {
    let h = setup();
    let first = h.schedule_set_value(1, 11);
    let second = h.schedule_set_value_after(2, 22, &first);
    h.client.cancel(&h.canceller, &first);
    h.warp(DELAY);
    assert!(h.client.try_execute(&h.executor, &second).is_err());
    assert_eq!(h.target.value(), 0);
}

// ── self-administration ──────────────────────────────────────────────────────

#[test]
fn self_calls_reconfigure_the_timelock() {
    let h = setup();
    let e = &h.env;
    let new_proposer = Address::generate(e);

    let delay_op = h.schedule_self(1, "update_delay", vec![e, (2 * DELAY).into_val(e)]);
    let grant_op = h.schedule_self(
        2,
        "grant_role",
        vec![
            e,
            Role::Proposer.into_val(e),
            new_proposer.clone().into_val(e),
        ],
    );
    let revoke_op = h.schedule_self(
        3,
        "revoke_role",
        vec![
            e,
            Role::Proposer.into_val(e),
            h.proposer.clone().into_val(e),
        ],
    );
    let open_op = h.schedule_self(4, "set_open_executor", vec![e, true.into_val(e)]);

    h.warp(DELAY);

    h.client.execute(&h.executor, &delay_op);
    assert_eq!(h.client.get_min_delay(), 2 * DELAY);

    h.client.execute(&h.executor, &grant_op);
    assert!(h.client.has_role(&Role::Proposer, &new_proposer));

    h.client.execute(&h.executor, &revoke_op);
    assert!(!h.client.has_role(&Role::Proposer, &h.proposer));

    h.client.execute(&h.executor, &open_op);
    assert!(h.client.is_open_executor());

    // Every one of those operations ran.
    assert!(h.client.is_operation_done(&delay_op));
    assert!(h.client.is_operation_done(&grant_op));
    assert!(h.client.is_operation_done(&revoke_op));
    assert!(h.client.is_operation_done(&open_op));
}

#[test]
fn a_self_call_the_timelock_does_not_implement_is_refused() {
    let h = setup();
    let e = &h.env;
    let op = h.schedule_self(1, "not_a_function", vec![e]);
    h.warp(DELAY);
    assert!(h.client.try_execute(&h.executor, &op).is_err());
}

// ── the id ───────────────────────────────────────────────────────────────────

#[test]
fn the_operation_id_commits_to_the_batch_the_predecessor_and_the_salt() {
    let h = setup();
    let id = h
        .client
        .hash_operation(&h.targets(), &h.fns(), &h.args(42), &None, &h.salt(1));
    assert_eq!(id, h.schedule_set_value(1, 42));

    // A different payload, predecessor or salt gives a different id.
    let other = h
        .client
        .hash_operation(&h.targets(), &h.fns(), &h.args(43), &None, &h.salt(1));
    assert_ne!(id, other);
    let salted = h
        .client
        .hash_operation(&h.targets(), &h.fns(), &h.args(42), &None, &h.salt(2));
    assert_ne!(id, salted);
    let preceded = h.client.hash_operation(
        &h.targets(),
        &h.fns(),
        &h.args(42),
        &Some(h.salt(3)),
        &h.salt(1),
    );
    assert_ne!(id, preceded);
}

#[test]
fn the_stored_operation_is_the_spec_it_was_scheduled_with() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    let op = h.client.get_operation(&id).unwrap();
    assert_eq!(op.targets.len(), 1);
    assert_eq!(op.targets.get(0), Some(h.target.address.clone()));
    assert_eq!(op.fns.len(), 1);
    assert_eq!(op.fns.get(0), Some(Symbol::new(&h.env, "set_value")));
    assert_eq!(op.args.len(), 1);
    assert_eq!(op.salt, h.salt(1));
    assert!(!op.done);
    assert_eq!(op.ready_at, DELAY);
    // The spec the id was derived from is the same one that was stored.
    assert_eq!(
        h.client
            .hash_operation(&op.targets, &op.fns, &op.args, &None, &op.salt,),
        id
    );
}

// ── the timelock as the admin of another contract ────────────────────────────

#[test]
fn the_timelock_is_the_admin_of_the_contract_it_acts_on() {
    let h = setup();
    h.schedule_set_value(1, 42);
    h.warp(DELAY);
    // Only the executor's own authorization is supplied. The target's admin
    // is the timelock, and the timelock is its direct invoker, so the
    // target's own admin check passes without a second signature.
    h.env.set_auths(&[MockAuth {
        address: &h.executor,
        invoke: &MockAuthInvoke {
            contract: &h.client.address,
            fn_name: "execute",
            args: vec![
                &h.env,
                h.executor.clone().into_val(&h.env),
                h.salt(1).into_val(&h.env),
            ],
            sub_invokes: &[],
        },
    }]);
    h.client.execute(&h.executor, &h.salt(1));
    assert_eq!(h.target.value(), 42);
}
