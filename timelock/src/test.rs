#![cfg(test)]

use crate::{Call, Role, Timelock, TimelockClient};
use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke},
    vec, Address, BytesN, Env, IntoVal, Symbol, Val, Vec,
};

const DELAY: u64 = 86_400;

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

/// The test fixture.
///
/// The client is built on demand rather than stored: a client borrows the
/// `Env`, so keeping one in the same struct that owns the `Env` would make
/// the struct self-referential.
struct Harness {
    env: Env,
    timelock: Address,
    target: Address,
    proposer: Address,
    executor: Address,
    canceller: Address,
}

impl Harness {
    fn client(&self) -> TimelockClient<'_> {
        TimelockClient::new(&self.env, &self.timelock)
    }

    fn target_client(&self) -> TargetClient<'_> {
        TargetClient::new(&self.env, &self.target)
    }

    /// Authorize `address` for one call of the timelock.
    ///
    /// The mock has to name the exact invocation, arguments included, so
    /// that "the proposer may schedule" is a statement about the proposer
    /// and not about whoever happened to sign.
    fn authorize(&self, address: &Address, fn_name: &str, args: Vec<Val>) {
        self.env.mock_auths(&[MockAuth {
            address,
            invoke: &MockAuthInvoke {
                contract: &self.timelock,
                fn_name,
                args,
                sub_invokes: &[],
            },
        }]);
    }
}

fn setup() -> Harness {
    setup_with_executors(true)
}

/// The fixture, with or without a named executor.
fn setup_with_executors(named_executor: bool) -> Harness {
    let env = Env::default();
    let proposer = Address::generate(&env);
    let executor = Address::generate(&env);
    let canceller = Address::generate(&env);
    let timelock = env.register_contract(None, Timelock);
    let target = env.register_contract(None, Target);
    TargetClient::new(&env, &target).init(&timelock);
    TimelockClient::new(&env, &timelock).initialize(
        &DELAY,
        &vec![&env, proposer.clone()],
        &if named_executor {
            vec![&env, executor.clone()]
        } else {
            vec![&env]
        },
        &vec![&env, canceller.clone()],
    );
    Harness {
        env,
        timelock,
        target,
        proposer,
        executor,
        canceller,
    }
}

fn salt_of(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

impl Harness {
    /// A one-call batch on the target.
    fn set_value_call(&self, value: u32) -> Vec<Call> {
        vec![
            &self.env,
            Call {
                target: self.target.clone(),
                function: Symbol::new(&self.env, "set_value"),
                args: vec![&self.env, value.into_val(&self.env)],
            },
        ]
    }

    /// A one-call batch on the timelock itself.
    fn self_call(&self, function: &str, args: Vec<Val>) -> Vec<Call> {
        vec![
            &self.env,
            Call {
                target: self.timelock.clone(),
                function: Symbol::new(&self.env, function),
                args,
            },
        ]
    }

    /// The arguments of a `schedule` call, for the auth mock.
    fn schedule_args(
        &self,
        proposer: &Address,
        calls: &Vec<Call>,
        predecessor: &Option<BytesN<32>>,
        salt: &BytesN<32>,
        delay: u64,
    ) -> Vec<Val> {
        vec![
            &self.env,
            proposer.clone().into_val(&self.env),
            calls.clone().into_val(&self.env),
            predecessor.clone().into_val(&self.env),
            salt.clone().into_val(&self.env),
            delay.into_val(&self.env),
        ]
    }

    /// The arguments of an `execute` call, for the auth mock.
    fn execute_args(
        &self,
        executor: &Address,
        calls: &Vec<Call>,
        predecessor: &Option<BytesN<32>>,
        salt: &BytesN<32>,
    ) -> Vec<Val> {
        vec![
            &self.env,
            executor.clone().into_val(&self.env),
            calls.clone().into_val(&self.env),
            predecessor.clone().into_val(&self.env),
            salt.clone().into_val(&self.env),
        ]
    }

    /// The arguments of a `cancel` call, for the auth mock.
    fn cancel_args(&self, canceller: &Address, id: &BytesN<32>) -> Vec<Val> {
        vec![
            &self.env,
            canceller.clone().into_val(&self.env),
            id.clone().into_val(&self.env),
        ]
    }

    /// Schedule a `set_value` batch, as the proposer. Returns its id.
    fn schedule_set_value(&self, n: u8, value: u32) -> BytesN<32> {
        let calls = self.set_value_call(value);
        let salt = salt_of(&self.env, n);
        self.authorize(
            &self.proposer,
            "schedule",
            self.schedule_args(&self.proposer, &calls, &None, &salt, DELAY),
        );
        self.client()
            .schedule(&self.proposer, &calls, &None, &salt, &DELAY)
    }

    /// Schedule a batch that depends on `predecessor`. Returns its id.
    fn schedule_after(&self, n: u8, predecessor: &BytesN<32>, value: u32) -> BytesN<32> {
        let calls = self.set_value_call(value);
        let salt = salt_of(&self.env, n);
        self.authorize(
            &self.proposer,
            "schedule",
            self.schedule_args(
                &self.proposer,
                &calls,
                &Some(predecessor.clone()),
                &salt,
                DELAY,
            ),
        );
        self.client().schedule(
            &self.proposer,
            &calls,
            &Some(predecessor.clone()),
            &salt,
            &DELAY,
        )
    }

    /// Execute a ready operation, as the executor.
    fn execute(&self, calls: &Vec<Call>, salt: &BytesN<32>) {
        self.authorize(
            &self.executor,
            "execute",
            self.execute_args(&self.executor, calls, &None, salt),
        );
        self.client().execute(&self.executor, calls, &None, salt);
    }

    /// Cancel a pending operation, as `canceller`.
    fn cancel_as(&self, canceller: &Address, id: &BytesN<32>) {
        self.authorize(canceller, "cancel", self.cancel_args(canceller, id));
        self.client().cancel(canceller, id);
    }

    fn warp(&self, dt: u64) {
        let t = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(t + dt);
    }
}

// ── initialization ───────────────────────────────────────────────────────────

#[test]
fn initialize_records_the_roles_and_the_timing() {
    let h = setup();
    assert_eq!(h.client().get_min_delay(), DELAY);
    assert!(h.client().has_role(&Role::Proposer, &h.proposer));
    assert!(h.client().has_role(&Role::Executor, &h.executor));
    assert!(h.client().has_role(&Role::Canceller, &h.canceller));
    // A named executor means execution is not open.
    assert!(!h.client().is_open_executor());
}

#[test]
fn an_empty_executor_set_means_anyone_may_execute() {
    let h = setup_with_executors(false);
    assert!(h.client().is_open_executor());
    assert!(!h.client().has_role(&Role::Executor, &h.executor));
}

#[test]
fn initialize_runs_only_once() {
    let h = setup();
    assert!(h
        .client()
        .try_initialize(&DELAY, &vec![&h.env], &vec![&h.env], &vec![&h.env])
        .is_err());
}

// ── scheduling ───────────────────────────────────────────────────────────────

#[test]
fn schedule_records_a_pending_operation_under_its_content_id() {
    let h = setup();
    let calls = h.set_value_call(42);
    let salt = salt_of(&h.env, 1);
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(&h.proposer, &calls, &None, &salt, DELAY),
    );
    let id = h
        .client()
        .schedule(&h.proposer, &calls, &None, &salt, &DELAY);
    // The id is the hash of the payload, so it can be derived before the
    // batch is ever scheduled.
    assert_eq!(id, h.client().hash_operation(&calls, &None, &salt));
    assert!(h.client().is_operation(&id));
    assert!(h.client().is_operation_pending(&id));
    assert!(!h.client().is_operation_done(&id));
    assert!(!h.client().is_operation_ready(&id));
    // The status packs into one number: 0 unknown, 1 done, else ready-at.
    assert_eq!(
        h.client().get_timestamp(&id),
        h.env.ledger().timestamp() + DELAY
    );
    assert_eq!(h.client().get_timestamp(&salt_of(&h.env, 9)), 0);
}

#[test]
fn only_a_proposer_can_schedule() {
    let h = setup();
    let e = &h.env;
    let calls = h.set_value_call(1);
    let salt = salt_of(e, 1);
    // The canceller signs, but the proposer is the one who schedules.
    h.authorize(
        &h.canceller,
        "schedule",
        h.schedule_args(&h.canceller, &calls, &None, &salt, DELAY),
    );
    assert_eq!(
        h.client()
            .try_schedule(&h.canceller, &calls, &None, &salt, &DELAY),
        Err(Ok(crate::error::Error::Unauthorized.into()))
    );
    assert!(!h
        .client()
        .is_operation(&h.client().hash_operation(&calls, &None, &salt)));
}

#[test]
fn rejects_a_delay_below_the_minimum() {
    let h = setup();
    let e = &h.env;
    let calls = h.set_value_call(1);
    let salt = salt_of(e, 1);
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(&h.proposer, &calls, &None, &salt, DELAY - 1),
    );
    assert_eq!(
        h.client()
            .try_schedule(&h.proposer, &calls, &None, &salt, &(DELAY - 1)),
        Err(Ok(crate::error::Error::InsufficientDelay.into()))
    );
}

#[test]
fn rejects_an_empty_batch() {
    let h = setup();
    let e = &h.env;
    let calls: Vec<Call> = vec![e];
    let salt = salt_of(e, 1);
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(&h.proposer, &calls, &None, &salt, DELAY),
    );
    assert_eq!(
        h.client()
            .try_schedule(&h.proposer, &calls, &None, &salt, &DELAY),
        Err(Ok(crate::error::Error::EmptyOperation.into()))
    );
}

#[test]
fn rejects_a_duplicate_operation_id() {
    let h = setup();
    let calls = h.set_value_call(1);
    let salt = salt_of(&h.env, 1);
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(&h.proposer, &calls, &None, &salt, DELAY),
    );
    h.client()
        .schedule(&h.proposer, &calls, &None, &salt, &DELAY);
    // The mock authorization is spent by the first call, so the second one
    // is authorized again.
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(&h.proposer, &calls, &None, &salt, DELAY),
    );
    assert_eq!(
        h.client()
            .try_schedule(&h.proposer, &calls, &None, &salt, &DELAY),
        Err(Ok(crate::error::Error::AlreadyScheduled.into()))
    );
}

// ── executing ────────────────────────────────────────────────────────────────

#[test]
fn executes_at_the_delay_boundary_and_not_before() {
    let h = setup();
    let calls = h.set_value_call(42);
    let salt = salt_of(&h.env, 1);
    let id = h.schedule_set_value(1, 42);
    h.warp(DELAY - 1);
    h.authorize(
        &h.executor,
        "execute",
        h.execute_args(&h.executor, &calls, &None, &salt),
    );
    assert_eq!(
        h.client().try_execute(&h.executor, &calls, &None, &salt),
        Err(Ok(crate::error::Error::NotReady.into()))
    );
    assert!(!h.client().is_operation_ready(&id));
    // Exactly at the ready-at the operation may run: the boundary is
    // inclusive.
    h.warp(1);
    assert!(h.client().is_operation_ready(&id));
    h.execute(&calls, &salt);
    assert_eq!(h.target_client().value(), 42);
    assert!(h.client().is_operation_done(&id));
    // A done operation reports 1, not its ready-at.
    assert_eq!(h.client().get_timestamp(&id), 1);
}

#[test]
fn an_operation_never_runs_twice() {
    let h = setup();
    let calls = h.set_value_call(42);
    let salt = salt_of(&h.env, 1);
    let id = h.schedule_set_value(1, 42);
    h.warp(DELAY);
    h.execute(&calls, &salt);
    assert!(h.client().is_operation_done(&id));
    h.authorize(
        &h.executor,
        "execute",
        h.execute_args(&h.executor, &calls, &None, &salt),
    );
    assert_eq!(
        h.client().try_execute(&h.executor, &calls, &None, &salt),
        Err(Ok(crate::error::Error::NotPending.into()))
    );
}

#[test]
fn only_an_executor_can_execute() {
    let h = setup();
    let calls = h.set_value_call(42);
    let salt = salt_of(&h.env, 1);
    let id = h.schedule_set_value(1, 42);
    h.warp(DELAY);
    // The proposer signs, but the executor is the one who executes.
    h.authorize(
        &h.proposer,
        "execute",
        h.execute_args(&h.proposer, &calls, &None, &salt),
    );
    assert_eq!(
        h.client().try_execute(&h.proposer, &calls, &None, &salt),
        Err(Ok(crate::error::Error::Unauthorized.into()))
    );
    // The operation is untouched by the failed attempt.
    assert!(h.client().is_operation_pending(&id));
    h.execute(&calls, &salt);
    assert_eq!(h.target_client().value(), 42);
}

#[test]
fn an_open_executor_lets_anyone_execute() {
    let h = setup_with_executors(false);
    let calls = h.set_value_call(7);
    let salt = salt_of(&h.env, 1);
    let id = h.schedule_set_value(1, 7);
    h.warp(DELAY);
    // No signature at all: execution is open.
    h.env.set_auths(&[]);
    h.client().execute(&h.proposer, &calls, &None, &salt);
    assert_eq!(h.target_client().value(), 7);
    assert!(h.client().is_operation_done(&id));
}

#[test]
fn the_target_needs_no_signature_because_the_timelock_is_the_invoker() {
    let h = setup();
    let calls = h.set_value_call(7);
    let salt = salt_of(&h.env, 1);
    let id = h.schedule_set_value(1, 7);
    h.warp(DELAY);
    h.execute(&calls, &salt);
    assert_eq!(h.target_client().value(), 7);
    assert!(h.client().is_operation_done(&id));
}

// ── cancelling ───────────────────────────────────────────────────────────────

#[test]
fn only_a_canceller_can_cancel() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    let stranger = Address::generate(&h.env);
    h.authorize(&stranger, "cancel", h.cancel_args(&stranger, &id));
    assert_eq!(
        h.client().try_cancel(&stranger, &id),
        Err(Ok(crate::error::Error::Unauthorized.into()))
    );
    assert!(h.client().is_operation_pending(&id));
}

#[test]
fn cancelling_removes_the_operation() {
    let h = setup();
    let id = h.schedule_set_value(1, 42);
    h.cancel_as(&h.canceller, &id);
    assert!(!h.client().is_operation(&id));
    assert!(!h.client().is_operation_pending(&id));
    assert_eq!(h.client().get_timestamp(&id), 0);
}

#[test]
fn a_cancelled_operation_can_never_run() {
    let h = setup();
    let calls = h.set_value_call(42);
    let salt = salt_of(&h.env, 1);
    let id = h.schedule_set_value(1, 42);
    h.cancel_as(&h.canceller, &id);
    h.warp(DELAY);
    h.authorize(
        &h.executor,
        "execute",
        h.execute_args(&h.executor, &calls, &None, &salt),
    );
    assert_eq!(
        h.client().try_execute(&h.executor, &calls, &None, &salt),
        Err(Ok(crate::error::Error::NotPending.into()))
    );
}

#[test]
fn cancelling_an_unknown_or_finished_operation_fails() {
    let h = setup();
    let unknown = salt_of(&h.env, 9);
    h.authorize(
        &h.canceller,
        "cancel",
        h.cancel_args(&h.canceller, &unknown),
    );
    assert_eq!(
        h.client().try_cancel(&h.canceller, &unknown),
        Err(Ok(crate::error::Error::NotPending.into()))
    );
    let calls = h.set_value_call(42);
    let salt = salt_of(&h.env, 1);
    let id = h.schedule_set_value(1, 42);
    h.warp(DELAY);
    h.execute(&calls, &salt);
    h.authorize(&h.canceller, "cancel", h.cancel_args(&h.canceller, &id));
    assert_eq!(
        h.client().try_cancel(&h.canceller, &id),
        Err(Ok(crate::error::Error::NotPending.into()))
    );
}

// ── predecessors ─────────────────────────────────────────────────────────────

#[test]
fn a_dependent_cannot_run_before_its_predecessor() {
    let h = setup();
    let first_calls = h.set_value_call(10);
    let first_salt = salt_of(&h.env, 1);
    let first = h.schedule_set_value(1, 10);
    let second_calls = h.set_value_call(20);
    let second_salt = salt_of(&h.env, 2);
    let second = h.schedule_after(2, &first, 20);
    assert_eq!(
        h.client().get_operation(&second).unwrap().predecessor,
        first.clone()
    );
    h.warp(DELAY);
    assert!(!h.client().is_operation_ready(&second));
    h.authorize(
        &h.executor,
        "execute",
        h.execute_args(
            &h.executor,
            &second_calls,
            &Some(first.clone()),
            &second_salt,
        ),
    );
    assert_eq!(
        h.client().try_execute(
            &h.executor,
            &second_calls,
            &Some(first.clone()),
            &second_salt
        ),
        Err(Ok(crate::error::Error::PredecessorNotDone.into()))
    );
    h.execute(&first_calls, &first_salt);
    assert!(h.client().is_operation_ready(&second));
    h.authorize(
        &h.executor,
        "execute",
        h.execute_args(
            &h.executor,
            &second_calls,
            &Some(first.clone()),
            &second_salt,
        ),
    );
    h.client().execute(
        &h.executor,
        &second_calls,
        &Some(first.clone()),
        &second_salt,
    );
    assert_eq!(h.target_client().value(), 20);
}

#[test]
fn cancelling_a_predecessor_leaves_its_dependent_unexecutable() {
    let h = setup();
    let first = h.schedule_set_value(1, 10);
    let second_calls = h.set_value_call(20);
    let second_salt = salt_of(&h.env, 2);
    let second = h.schedule_after(2, &first, 20);
    h.cancel_as(&h.canceller, &first);
    h.warp(DELAY);
    // The predecessor is gone, so the dependent can never run.
    h.authorize(
        &h.executor,
        "execute",
        h.execute_args(
            &h.executor,
            &second_calls,
            &Some(first.clone()),
            &second_salt,
        ),
    );
    assert_eq!(
        h.client().try_execute(
            &h.executor,
            &second_calls,
            &Some(first.clone()),
            &second_salt
        ),
        Err(Ok(crate::error::Error::PredecessorNotDone.into()))
    );
    assert!(!h.client().is_operation_ready(&second));
}

// ── self-administration ──────────────────────────────────────────────────────

#[test]
fn self_calls_reconfigure_the_timelock() {
    let h = setup();
    let new_proposer = Address::generate(&h.env);

    // Each self-call is scheduled and run on its own: the id is the hash of
    // the payload, so the executor has to repeat exactly the same call.
    let update_delay_calls =
        h.self_call("update_delay", vec![&h.env, (2 * DELAY).into_val(&h.env)]);
    let update_delay_salt = salt_of(&h.env, 1);
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(
            &h.proposer,
            &update_delay_calls,
            &None,
            &update_delay_salt,
            DELAY,
        ),
    );
    h.client().schedule(
        &h.proposer,
        &update_delay_calls,
        &None,
        &update_delay_salt,
        &DELAY,
    );

    let grant_calls = h.self_call(
        "grant_role",
        vec![
            &h.env,
            Role::Proposer.into_val(&h.env),
            new_proposer.clone().into_val(&h.env),
        ],
    );
    let grant_salt = salt_of(&h.env, 2);
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(&h.proposer, &grant_calls, &None, &grant_salt, DELAY),
    );
    h.client()
        .schedule(&h.proposer, &grant_calls, &None, &grant_salt, &DELAY);

    let revoke_calls = h.self_call(
        "revoke_role",
        vec![
            &h.env,
            Role::Canceller.into_val(&h.env),
            h.canceller.clone().into_val(&h.env),
        ],
    );
    let revoke_salt = salt_of(&h.env, 3);
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(&h.proposer, &revoke_calls, &None, &revoke_salt, DELAY),
    );
    h.client()
        .schedule(&h.proposer, &revoke_calls, &None, &revoke_salt, &DELAY);

    let open_calls = h.self_call("set_open_executor", vec![&h.env, true.into_val(&h.env)]);
    let open_salt = salt_of(&h.env, 4);
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(&h.proposer, &open_calls, &None, &open_salt, DELAY),
    );
    h.client()
        .schedule(&h.proposer, &open_calls, &None, &open_salt, &DELAY);

    h.warp(DELAY);
    for (calls, s) in [
        (&update_delay_calls, &update_delay_salt),
        (&grant_calls, &grant_salt),
        (&revoke_calls, &revoke_salt),
        (&open_calls, &open_salt),
    ] {
        h.authorize(
            &h.executor,
            "execute",
            h.execute_args(&h.executor, calls, &None, s),
        );
        h.client().execute(&h.executor, calls, &None, s);
    }

    assert_eq!(h.client().get_min_delay(), 2 * DELAY);
    assert!(h.client().has_role(&Role::Proposer, &new_proposer));
    assert!(!h.client().has_role(&Role::Canceller, &h.canceller));
    assert!(h.client().is_open_executor());
}

#[test]
fn an_unknown_self_call_is_refused() {
    let h = setup();
    let calls = h.self_call("nope", vec![&h.env]);
    let salt = salt_of(&h.env, 1);
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(&h.proposer, &calls, &None, &salt, DELAY),
    );
    h.client()
        .schedule(&h.proposer, &calls, &None, &salt, &DELAY);
    h.warp(DELAY);
    h.authorize(
        &h.executor,
        "execute",
        h.execute_args(&h.executor, &calls, &None, &salt),
    );
    assert_eq!(
        h.client().try_execute(&h.executor, &calls, &None, &salt),
        Err(Ok(crate::error::Error::UnknownSelfCall.into()))
    );
}

// ── migrations shim ──────────────────────────────────────────────────────────

#[test]
fn the_migrations_shim_still_works() {
    let h = setup();
    assert_eq!(
        h.client().schema_version(),
        crate::migrations::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(h.client().migration_state(), None);
    // Nothing is in progress, so a batch cannot be continued.
    assert_eq!(
        h.client().try_migrate_batch(&0, &50),
        Err(Ok(crate::error::Error::NoMigrationInProgress.into()))
    );
}

#[test]
fn the_migrate_self_call_reaches_the_migration_framework() {
    let h = setup();
    // `migrate(from, to)` with `to <= from` is an invalid target, which
    // proves the self-call really dispatches into the migration framework
    // rather than being ignored.
    let calls = h.self_call(
        "migrate",
        vec![&h.env, 1u32.into_val(&h.env), 1u32.into_val(&h.env)],
    );
    let salt = salt_of(&h.env, 1);
    h.authorize(
        &h.proposer,
        "schedule",
        h.schedule_args(&h.proposer, &calls, &None, &salt, DELAY),
    );
    h.client()
        .schedule(&h.proposer, &calls, &None, &salt, &DELAY);
    h.warp(DELAY);
    h.authorize(
        &h.executor,
        "execute",
        h.execute_args(&h.executor, &calls, &None, &salt),
    );
    assert_eq!(
        h.client().try_execute(&h.executor, &calls, &None, &salt),
        Err(Ok(crate::error::Error::InvalidMigrationTarget.into()))
    );
}
