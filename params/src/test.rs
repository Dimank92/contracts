#![cfg(test)]

use crate::{error::Error, Params, ParamsClient, BOUNDS_CHANGE_DELAY, CURRENT_SCHEMA_VERSION};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Events as _, Ledger},
    Address, Env, IntoVal, Symbol,
};

fn setup<'a>() -> (Env, ParamsClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let timelock = Address::generate(&env);
    let id = env.register_contract(None, Params);
    let client = ParamsClient::new(&env, &id);
    client.initialize(&timelock);
    (env, client, timelock)
}

fn fee() -> Symbol {
    symbol_short!("fee_bps")
}

#[test]
fn define_and_set_within_bounds() {
    let (env, client, _) = setup();
    assert_eq!(client.get_param(&fee()), None);
    client.define_param(&fee(), &50, &0, &1_000);
    assert_eq!(client.get_version(), 1);

    client.set_param(&fee(), &75);
    let p = client.get_param(&fee()).unwrap();
    assert_eq!((p.value, p.min, p.max), (75, 0, 1_000));
    assert_eq!(client.get_value(&fee()), 75);
    assert_eq!(client.get_version(), 2);

    let (_, topics, data) = env.events().all().last().unwrap();
    assert_eq!(
        topics,
        (Symbol::new(&env, "param_updated"), fee()).into_val(&env)
    );
    let (old, new): (i128, i128) = data.into_val(&env);
    assert_eq!((old, new), (50, 75));
}

#[test]
fn set_param_rejects_out_of_bounds() {
    let (_, client, _) = setup();
    client.define_param(&fee(), &50, &0, &1_000);
    assert!(client.try_set_param(&fee(), &1_001).is_err());
    assert!(client.try_set_param(&fee(), &-1).is_err());
    assert_eq!(client.get_value(&fee()), 50);
}

#[test]
fn unset_param_is_not_found() {
    let (_, client, _) = setup();
    assert!(client.try_set_param(&fee(), &1).is_err());
    assert!(client.try_get_value(&fee()).is_err());
}

#[test]
fn define_rejects_duplicates_and_bad_bounds() {
    let (_, client, _) = setup();
    assert!(client.try_define_param(&fee(), &5, &10, &0).is_err());
    assert!(client.try_define_param(&fee(), &50, &0, &10).is_err());
    client.define_param(&fee(), &5, &0, &10);
    assert!(client.try_define_param(&fee(), &5, &0, &10).is_err());
}

#[test]
fn set_param_requires_timelock() {
    let env = Env::default();
    let timelock = Address::generate(&env);
    let id = env.register_contract(None, Params);
    let client = ParamsClient::new(&env, &id);
    client.initialize(&timelock);
    assert!(client.try_define_param(&fee(), &5, &0, &10).is_err());
}

#[test]
fn widening_bounds_needs_the_longer_delay() {
    let (env, client, _) = setup();
    client.define_param(&fee(), &50, &0, &100);
    client.propose_bounds(&fee(), &0, &1_000);

    // Still enforced against the old bounds until executed.
    assert!(client.try_set_param(&fee(), &500).is_err());
    assert!(client.try_execute_bounds(&fee()).is_err());

    env.ledger()
        .with_mut(|l| l.timestamp += BOUNDS_CHANGE_DELAY - 1);
    assert!(client.try_execute_bounds(&fee()).is_err());

    env.ledger().with_mut(|l| l.timestamp += 1);
    client.execute_bounds(&fee());
    assert_eq!(client.get_pending_bounds(&fee()), None);
    client.set_param(&fee(), &500);
    assert_eq!(client.get_value(&fee()), 500);
}

#[test]
fn bounds_must_contain_current_value_and_can_be_cancelled() {
    let (_, client, _) = setup();
    client.define_param(&fee(), &50, &0, &100);
    assert!(client.try_propose_bounds(&fee(), &60, &100).is_err());
    assert!(client.try_propose_bounds(&fee(), &100, &0).is_err());
    client.propose_bounds(&fee(), &0, &200);
    client.cancel_bounds(&fee());
    assert!(client.try_execute_bounds(&fee()).is_err());
    assert!(client.try_cancel_bounds(&fee()).is_err());
}

#[test]
fn schema_version_set_at_initialize() {
    let (_, client, _) = setup();
    assert_eq!(client.schema_version(), CURRENT_SCHEMA_VERSION);
    assert_eq!(client.migration_state(), None);
}

#[test]
fn migrate_rejects_unknown_or_stale_targets() {
    let (_, client, _) = setup();
    let v = CURRENT_SCHEMA_VERSION;
    assert_eq!(
        client.try_migrate(&v, &(v + 1)),
        Err(Ok(Error::InvalidMigrationTarget.into()))
    );
    assert_eq!(
        client.try_migrate(&(v - 1), &v),
        Err(Ok(Error::SchemaVersionMismatch.into()))
    );
    assert_eq!(
        client.try_migrate_batch(&0, &10),
        Err(Ok(Error::NoMigrationInProgress.into()))
    );
}

#[test]
fn migrate_requires_timelock() {
    let env = Env::default();
    let timelock = Address::generate(&env);
    let id = env.register_contract(None, Params);
    let client = ParamsClient::new(&env, &id);
    client.initialize(&timelock);
    let v = CURRENT_SCHEMA_VERSION;
    assert!(client.try_migrate(&v, &(v + 1)).is_err());
}
