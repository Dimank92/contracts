#![cfg(test)]

use crate::{
    Error, ParamUpdatedEvent, Params, ParamsClient, BOUNDS_CHANGE_DELAY, CURRENT_SCHEMA_VERSION,
};
use soroban_sdk::{
    symbol_short,
    testutils::{Address as _, Ledger},
    Address, Env, IntoVal, Symbol, TryFromVal,
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

    let (topics, data) = event_spec::last_event(&env, &client.address);
    assert_eq!(
        topics,
        (Symbol::new(&env, "param_updated"), 2u32, fee()).into_val(&env)
    );
    let data = ParamUpdatedEvent::try_from_val(&env, &data).unwrap();
    assert_eq!((data.old_value, data.new_value, data.version), (50, 75, 2));
}

#[test]
fn set_param_rejects_out_of_bounds() {
    let (_, client, _) = setup();
    client.define_param(&fee(), &50, &0, &1_000);
    assert_eq!(
        client.try_set_param(&fee(), &1_001),
        Err(Ok(Error::OutOfBounds))
    );
    assert_eq!(
        client.try_set_param(&fee(), &-1),
        Err(Ok(Error::OutOfBounds))
    );
    assert_eq!(client.get_value(&fee()), 50);
}

#[test]
fn unset_param_is_not_found() {
    let (_, client, _) = setup();
    assert_eq!(
        client.try_set_param(&fee(), &1),
        Err(Ok(Error::ParamNotFound))
    );
    assert_eq!(client.try_get_value(&fee()), Err(Ok(Error::ParamNotFound)));
}

#[test]
fn define_rejects_duplicates_and_bad_bounds() {
    let (_, client, _) = setup();
    assert_eq!(
        client.try_define_param(&fee(), &5, &10, &0),
        Err(Ok(Error::InvalidBounds))
    );
    assert_eq!(
        client.try_define_param(&fee(), &50, &0, &10),
        Err(Ok(Error::OutOfBounds))
    );
    client.define_param(&fee(), &5, &0, &10);
    assert_eq!(
        client.try_define_param(&fee(), &5, &0, &10),
        Err(Ok(Error::ParamAlreadyDefined))
    );
}

#[test]
fn set_param_requires_timelock() {
    let env = Env::default();
    let timelock = Address::generate(&env);
    let id = env.register_contract(None, Params);
    let client = ParamsClient::new(&env, &id);
    client.initialize(&timelock);
    // No signature from the timelock: the host rejects the auth check.
    assert!(matches!(
        client.try_define_param(&fee(), &5, &0, &10),
        Err(Err(_))
    ));
}

#[test]
fn second_initialize_is_rejected() {
    let (env, client, _) = setup();
    assert_eq!(
        client.try_initialize(&Address::generate(&env)),
        Err(Ok(Error::AlreadyInitialized))
    );
}

#[test]
fn uninitialized_registry_reports_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let client = ParamsClient::new(&env, &env.register_contract(None, Params));
    assert_eq!(client.try_get_timelock(), Err(Ok(Error::NotInitialized)));
    assert_eq!(
        client.try_define_param(&fee(), &5, &0, &10),
        Err(Ok(Error::NotInitialized))
    );
}

#[test]
fn widening_bounds_needs_the_longer_delay() {
    let (env, client, _) = setup();
    client.define_param(&fee(), &50, &0, &100);
    client.propose_bounds(&fee(), &0, &1_000);

    // Still enforced against the old bounds until executed.
    assert_eq!(
        client.try_set_param(&fee(), &500),
        Err(Ok(Error::OutOfBounds))
    );
    assert_eq!(
        client.try_execute_bounds(&fee()),
        Err(Ok(Error::BoundsDelayNotElapsed))
    );

    env.ledger()
        .with_mut(|l| l.timestamp += BOUNDS_CHANGE_DELAY - 1);
    assert_eq!(
        client.try_execute_bounds(&fee()),
        Err(Ok(Error::BoundsDelayNotElapsed))
    );

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
    assert_eq!(
        client.try_propose_bounds(&fee(), &60, &100),
        Err(Ok(Error::OutOfBounds))
    );
    assert_eq!(
        client.try_propose_bounds(&fee(), &100, &0),
        Err(Ok(Error::InvalidBounds))
    );
    client.propose_bounds(&fee(), &0, &200);
    client.cancel_bounds(&fee());
    assert_eq!(
        client.try_execute_bounds(&fee()),
        Err(Ok(Error::NoPendingBounds))
    );
    assert_eq!(
        client.try_cancel_bounds(&fee()),
        Err(Ok(Error::NoPendingBounds))
    );
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
    assert!(matches!(client.try_migrate(&v, &(v + 1)), Err(Err(_))));
}
