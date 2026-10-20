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

    fn target_targets(&self) -> Vec<Address> {
        vec![&self.env, self.target.address.clone()]
    }

    fn set_value_fns(&self) -> Vec<Symbol> {
        vec![&self.env, Symbol::new(&self.env, "set_value")]
    }

    fn set_value_args(&self, value: u32) -> Vec<Vec<Val>> {
        vec![&self.env, vec![&self.env, value.into_val(&self.env)]]
    }

    /// The arguments of the `execute` call, as the host sees them.
    fn execute_args(&self, n: u8, value: u32) -> Vec<Val> {
        vec![
            &self.env,
            self.executor.clone().into_val(&self.env),
            self.target_targets().into_val(&self.env),
            self.set_value_fns().into_val(&self.env),
            self.set_value_args(value).into_val(&self.env),
            None::<BytesN<32>>.into_val(&self.env),
            self.salt(n).into_val(&self.env),
        ]
    }

    fn schedule_set_value(&self, n: u8, value: u32) -> BytesN<32> {
        self.client.schedule(
            &self.proposer,
            &self.target_targets(),
            &self.set_value_fns(),
            &self.set_value_args(value),
            &None,
            &self.salt(n),
            &DELAY,
        )
    }

    /// Schedules a call whose target is the timelock itself.
    fn schedule_self(&self, n: u8, func: &str, args: Vec<Val>) -> BytesN<32> {
        self.client.schedule(
            &self.proposer,
            &vec![&self.env, self.client.address.clone()],
            &vec![&self.env, Symbol::new(&self.env, func)],
            &vec![&self.env, args],
            &None,
            &self.salt(n),
            &DELAY,
        )
    }

    fn execute_set_value(&self, n: u8, value: u32) {
        self.client.execute(
            &self.executor,
            &self.target_targets(),
            &self.set_value_fns(),
            &self.set_value_args(value),
            &None,
            &self.salt(n),
        );
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
    let h = setup();
    assert!(!h.client.is_open_executor());
    let e = &h.env;
    let client = TimelockClient::new(&e, &e.register_contract(None, Timelock));
    client.initialize(
        &DELAY,
        &GRACE,
        &vec![&e, h.proposer.clone()],
        &vec![&e],
        &vec![&e],
    );
    assert!(client.is_open_executor());
}

// ── role gating ──────────────────────────────────────────────────────────────

#[test]
fn only_a_proposer_may_schedule() {
    let h = setup();
    assert!(h
        .client
        .try_schedule(
            &h.outsider,
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(1),
            &None,
            &h.salt(1),
            &DELAY,
        )
        .is_err());
    assert!(h
        .client
        .try_schedule(
            &h.canceller,
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(1),
            &None,
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
    assert!(h
        .client
        .try_execute(
            &h.outsider,
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(42),
            &None,
            &h.salt(1),
        )
        .is_err());
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
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(1),
            &None,
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
    assert!(h
        .client
        .try_execute(
            &h.executor,
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(42),
            &None,
            &h.salt(1),
        )
        .is_err());

    h.warp(1);
    assert!(h.client.is_operation_ready(&id));
    h.execute_set_value(1, 42);
    assert_eq!(h.target.value(), 42);
}

// ── lifecycle ────────────────────────────────────────────────────────────────

#[test]
fn an_operation_cannot_execute_twice() {
    let h = setup();
    h.schedule_set_value(1, 42);
    h.warp(DELAY);
    h.execute_set_value(1, 42);
    assert!(h.client.is_operation_done(&h.salt(1)));
    assert!(h
        .client
        .try_execute(
            &h.executor,
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(42),
            &None,
            &h.salt(1),
        )
        .is_err());
}

#[test]
fn cancelling_before_execution_stops_it() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    h.client.cancel(&h.canceller, &id);
    h.warp(DELAY);
    assert!(h
        .client
        .try_execute(
            &h.executor,
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(42),
            &None,
            &h.salt(1),
        )
        .is_err());
    assert_eq!(h.target.value(), 0);
}

#[test]
fn executing_after_a_cancel_is_refused() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    h.client.cancel(&h.canceller, &id);
    assert!(h
        .client
        .try_execute(
            &h.executor,
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(42),
            &None,
            &h.salt(1),
        )
        .is_err());
}

#[test]
fn cancelling_an_already_executed_operation_is_refused() {
    let h = setup();
    h.schedule_set_value(1, 42);
    h.warp(DELAY);
    h.execute_set_value(1, 42);
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
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(42),
            &None,
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
            &None,
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
    let second = h.client.schedule(
        &h.proposer,
        &h.target_targets(),
        &h.set_value_fns(),
        &h.set_value_args(22),
        &Some(first.clone()),
        &h.salt(2),
        &DELAY,
    );
    h.warp(DELAY);

    // The dependant is ready in time, but its predecessor has not run.
    assert!(h.client.is_operation_ready(&second));
    assert!(h
        .client
        .try_execute(
            &h.executor,
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(22),
            &Some(first.clone()),
            &h.salt(2),
        )
        .is_err());
    assert_eq!(h.target.value(), 0);

    h.execute_set_value(1, 11);
    h.client.execute(
        &h.executor,
        &h.target_targets(),
        &h.set_value_fns(),
        &h.set_value_args(22),
        &Some(first.clone()),
        &h.salt(2),
    );
    assert_eq!(h.target.value(), 22);
}

#[test]
fn cancelling_a_predecessor_strands_its_dependant() {
    let h = setup();
    let first = h.schedule_set_value(1, 11);
    h.client.schedule(
        &h.proposer,
        &h.target_targets(),
        &h.set_value_fns(),
        &h.set_value_args(22),
        &Some(first.clone()),
        &h.salt(2),
        &DELAY,
    );
    h.client.cancel(&h.canceller, &first);
    h.warp(DELAY);
    assert!(h
        .client
        .try_execute(
            &h.executor,
            &h.target_targets(),
            &h.set_value_fns(),
            &h.set_value_args(22),
            &Some(first.clone()),
            &h.salt(2),
        )
        .is_err());
    assert_eq!(h.target.value(), 0);
}

// ── self-administration ──────────────────────────────────────────────────────

#[test]
fn self_calls_reconfigure_the_timelock() {
    let h = setup();
    let new_proposer = Address::generate(&h.env);
    let e = &h.env;

    let delay_args = vec![&e, (2 * DELAY).into_val(&e)];
    let grant_args = vec![
        &e,
        Role::Proposer.into_val(&e),
        new_proposer.clone().into_val(&e),
    ];
    let revoke_args = vec![
        &e,
        Role::Proposer.into_val(&e),
        h.proposer.clone().into_val(&e),
    ];
    let open_args = vec![&e, true.into_val(&e)];

    let delay_op = h.schedule_self(1, "update_delay", delay_args.clone());
    let grant_op = h.schedule_self(2, "grant_role", grant_args.clone());
    let revoke_op = h.schedule_self(3, "revoke_role", revoke_args.clone());
    let open_op = h.schedule_self(4, "set_open_executor", open_args.clone());

    h.warp(DELAY);
    h.client.execute(
        &h.executor,
        &vec![&e, h.client.address.clone()],
        &vec![&e, Symbol::new(&e, "update_delay")],
        &vec![&e, delay_args.clone()],
        &None,
        &h.salt(1),
    );
    assert_eq!(h.client.get_min_delay(), 2 * DELAY);

    h.client.execute(
        &h.executor,
        &vec![&e, h.client.address.clone()],
        &vec![&e, Symbol::new(&e, "grant_role")],
        &vec![&e, grant_args.clone()],
        &None,
        &h.salt(2),
    );
    assert!(h.client.has_role(&Role::Proposer, &new_proposer));

    h.client.execute(
        &h.executor,
        &vec![&e, h.client.address.clone()],
        &vec![&e, Symbol::new(&e, "revoke_role")],
        &vec![&e, revoke_args.clone()],
        &None,
        &h.salt(3),
    );
    assert!(!h.client.has_role(&Role::Proposer, &h.proposer));

    h.client.execute(
        &h.executor,
        &vec![&e, h.client.address.clone()],
        &vec![&e, Symbol::new(&e, "set_open_executor")],
        &vec![&e, open_args.clone()],
        &None,
        &h.salt(4),
    );
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
    h.schedule_self(1, "not_a_function", vec![&e]);
    h.warp(DELAY);
    assert!(h
        .client
        .try_execute(
            &h.executor,
            &vec![&e, h.client.address.clone()],
            &vec![&e, Symbol::new(&e, "not_a_function")],
            &vec![&e, vec![&e]],
            &None,
            &h.salt(1),
        )
        .is_err());
}

// ── the id ───────────────────────────────────────────────────────────────────

#[test]
fn the_operation_id_commits_to_the_batch_the_predecessor_and_the_salt() {
    let h = setup();
    let id = h.client.hash_operation(
        &h.target_targets(),
        &h.set_value_fns(),
        &h.set_value_args(42),
        &None,
        &h.salt(1),
    );
    assert_eq!(id, h.schedule_set_value(1, 42));

    // A different payload or a different salt gives a different id.
    let other = h.client.hash_operation(
        &h.target_targets(),
        &h.set_value_fns(),
        &h.set_value_args(43),
        &None,
        &h.salt(1),
    );
    assert_ne!(id, other);
    let salted = h.client.hash_operation(
        &h.target_targets(),
        &h.set_value_fns(),
        &h.set_value_args(42),
        &None,
        &h.salt(2),
    );
    assert_ne!(id, salted);
    let preceded = h.client.hash_operation(
        &h.target_targets(),
        &h.set_value_fns(),
        &h.set_value_args(42),
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
    h.env.mock_auths(&[MockAuth {
        address: &h.executor,
        invoke: &MockAuthInvoke {
            contract: &h.client.address,
            fn_name: "execute",
            args: h.execute_args(1, 42),
            sub_invokes: &[],
        },
    }]);
    h.client.execute(
        &h.executor,
        &h.target_targets(),
        &h.set_value_fns(),
        &h.set_value_args(42),
        &None,
        &h.salt(1),
    );
    assert_eq!(h.target.value(), 42);
}
