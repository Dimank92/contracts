#![cfg(test)]

use crate::{Timelock, TimelockClient};
use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::{Address as _, Ledger as _},
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
    guardian: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    let proposer = Address::generate(&env);
    let guardian = Address::generate(&env);
    let client = TimelockClient::new(&env, &env.register_contract(None, Timelock));
    client.initialize(&proposer, &guardian, &DELAY, &GRACE);
    let target = TargetClient::new(&env, &env.register_contract(None, Target));
    target.init(&client.address);
    Harness {
        env,
        client,
        target,
        proposer,
        guardian,
    }
}

fn id(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

impl Harness<'_> {
    fn schedule_set_value(&self, n: u8, value: u32) -> BytesN<32> {
        let op = id(&self.env, n);
        self.client.schedule(
            &op,
            &vec![&self.env, self.target.address.clone()],
            &vec![&self.env, Symbol::new(&self.env, "set_value")],
            &vec![&self.env, vec![&self.env, value.into_val(&self.env)]],
            &DELAY,
        );
        op
    }

    fn schedule_self(&self, n: u8, func: Symbol, args: Vec<Val>) -> BytesN<32> {
        let op = id(&self.env, n);
        self.client.schedule(
            &op,
            &vec![&self.env, self.client.address.clone()],
            &vec![&self.env, func],
            &vec![&self.env, args],
            &DELAY,
        );
        op
    }

    fn warp(&self, dt: u64) {
        let t = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(t + dt);
    }
}

#[test]
fn executes_after_delay_as_admin() {
    let h = setup();
    let op = h.schedule_set_value(1, 42);
    assert!(h.client.try_execute(&op).is_err());
    h.warp(DELAY);
    h.client.execute(&op);
    assert_eq!(h.target.value(), 42);
    assert!(h.client.get_operation(&op).unwrap().done);
    assert!(h.client.try_execute(&op).is_err());
}

#[test]
fn anyone_executes_and_target_needs_no_signature() {
    let h = setup();
    let op = h.schedule_set_value(1, 7);
    h.warp(DELAY);
    // No signatures at all: execution is open, and the target's admin
    // check is satisfied because the timelock is its direct invoker.
    h.env.set_auths(&[]);
    h.client.execute(&op);
    assert_eq!(h.target.value(), 7);
    assert!(h.client.is_done(&op));
}

#[test]
fn rejects_short_delay_and_duplicates() {
    let h = setup();
    let e = &h.env;
    let targets = vec![e, h.target.address.clone()];
    let fns = vec![e, Symbol::new(e, "set_value")];
    let args = vec![e, vec![e, 1u32.into_val(e)]];
    assert!(h
        .client
        .try_schedule(&id(e, 1), &targets, &fns, &args, &(DELAY - 1))
        .is_err());
    h.client.schedule(&id(e, 1), &targets, &fns, &args, &DELAY);
    assert!(h
        .client
        .try_schedule(&id(e, 1), &targets, &fns, &args, &DELAY)
        .is_err());
    assert!(h
        .client
        .try_schedule(&id(e, 2), &targets, &vec![e], &args, &DELAY)
        .is_err());
}

#[test]
fn expires_after_grace_period() {
    let h = setup();
    let op = h.schedule_set_value(1, 42);
    h.warp(DELAY + GRACE + 1);
    assert!(h.client.try_execute(&op).is_err());
}

#[test]
fn guardian_and_proposer_can_cancel_others_cannot() {
    let h = setup();
    let op = h.schedule_set_value(1, 42);
    assert!(h
        .client
        .try_cancel(&Address::generate(&h.env), &op)
        .is_err());
    h.client.cancel(&h.guardian, &op);
    assert!(h.client.get_operation(&op).is_none());
    let op = h.schedule_set_value(2, 42);
    h.client.cancel(&h.proposer, &op);
    h.warp(DELAY);
    assert!(h.client.try_execute(&op).is_err());
}

#[test]
fn self_calls_reconfigure_and_lock_in() {
    let h = setup();
    let new_proposer = Address::generate(&h.env);
    let op1 = h.schedule_self(
        1,
        symbol_short!("set_delay"),
        vec![&h.env, (2 * DELAY).into_val(&h.env)],
    );
    let op2 = h.schedule_self(2, symbol_short!("lock_in"), vec![&h.env]);
    let op3 = h.schedule_self(
        3,
        symbol_short!("set_prop"),
        vec![&h.env, new_proposer.into_val(&h.env)],
    );
    h.warp(DELAY);
    h.client.execute(&op1);
    assert_eq!(h.client.get_min_delay(), 2 * DELAY);
    h.client.execute(&op2);
    assert!(h.client.is_locked_in());
    assert_eq!(h.client.get_guardian(), None);
    assert!(h.client.try_guardian_set_proposer(&h.guardian).is_err());
    h.client.execute(&op3);
    assert_eq!(h.client.get_proposer(), new_proposer);
}

#[test]
fn guardian_rollback_before_lock_in() {
    let h = setup();
    let fallback = Address::generate(&h.env);
    h.client.guardian_set_proposer(&fallback);
    assert_eq!(h.client.get_proposer(), fallback);
}
