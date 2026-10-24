//! Adversarial suite for the `timelock` contract.
//!
//! Focus areas:
//! * Predecessor-ordering bypass: an executor must not be able to run a
//!   proposal before its declared predecessor has executed.
//! * Role bypass: only addresses holding the executor role may execute
//!   proposals; unauthorized callers must be rejected.
//!
//! All scenarios are deterministic and drive the contract exclusively through
//! its public entrypoints. No storage back doors are used.

use soroban_sdk::{
    testutils::{Address as _, Ledger, LedgerInfo},
    Address, BytesN, Env, Vec,
};

use timelock::{TimelockContract, TimelockContractClient, ProposalState};

/// Fixed ledger timestamp used across the suite so tests never depend on the
/// wall clock.
const START_TS: u64 = 1_000_000;

fn setup<'a>() -> (Env, TimelockContractClient<'a>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    env.ledger().set(LedgerInfo {
        timestamp: START_TS,
        protocol_version: 20,
        sequence_number: 1,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 16,
        max_entry_ttl: 6_312_000,
    });

    let admin = Address::generate(&env);
    let executor = Address::generate(&env);

    let contract_id = env.register_contract(None, TimelockContract);
    let client = TimelockContractClient::new(&env, &contract_id);

    client.initialize(&admin, &executor);

    (env, client, admin, executor)
}

/// Builds a deterministic 32-byte identifier from a single byte seed.
fn id(env: &Env, seed: u8) -> BytesN<32> {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    BytesN::from_array(env, &bytes)
}

/// A proposal whose predecessor has not executed must not be executable.
#[test]
fn predecessor_not_executed_blocks_execution() {
    let (env, client, _admin, _executor) = setup();

    let first = id(&env, 1);
    let second = id(&env, 2);

    // Schedule the predecessor but do not execute it.
    client.schedule(&first, &None, &(START_TS + 100));

    // Schedule the dependent proposal referencing the unexecuted predecessor.
    client.schedule(&second, &Some(first.clone()), &(START_TS + 100));

    // Advance past the delay so timing is not the reason for rejection.
    env.ledger().set(LedgerInfo {
        timestamp: START_TS + 1_000,
        protocol_version: 20,
        sequence_number: 2,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 16,
        max_entry_ttl: 6_312_000,
    });

    // The dependent proposal must still be blocked by its predecessor.
    let result = client.try_execute(&second);
    assert!(
        result.is_err(),
        "dependent proposal executed before its predecessor"
    );

    // The predecessor itself is now executable.
    client.execute(&first);
    assert_eq!(client.get_state(&first), ProposalState::Executed);

    // Once the predecessor has executed, the dependent proposal may run.
    client.execute(&second);
    assert_eq!(client.get_state(&second), ProposalState::Executed);
}

/// A proposal cannot be executed before its scheduled delay has elapsed.
#[test]
fn execution_before_delay_is_rejected() {
    let (env, client, _admin, _executor) = setup();

    let proposal = id(&env, 10);
    client.schedule(&proposal, &None, &(START_TS + 500));

    // Still within the delay window.
    let result = client.try_execute(&proposal);
    assert!(result.is_err(), "proposal executed before delay elapsed");

    // Advance past the delay.
    env.ledger().set(LedgerInfo {
        timestamp: START_TS + 501,
        protocol_version: 20,
        sequence_number: 2,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 16,
        max_entry_ttl: 6_312_000,
    });

    client.execute(&proposal);
    assert_eq!(client.get_state(&proposal), ProposalState::Executed);
}

/// An address without the executor role must not be able to execute a
/// proposal, even after the delay has elapsed.
#[test]
fn non_executor_cannot_execute() {
    let (env, client, _admin, _executor) = setup();

    let proposal = id(&env, 20);
    client.schedule(&proposal, &None, &(START_TS + 100));

    env.ledger().set(LedgerInfo {
        timestamp: START_TS + 1_000,
        protocol_version: 20,
        sequence_number: 2,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 16,
        max_entry_ttl: 6_312_000,
    });

    // Drop the blanket auth mock so the caller's authorization is checked.
    env.mock_auths(&[]);

    let attacker = Address::generate(&env);
    let result = client
        .mock_auths(&[])
        .try_execute(&proposal);
    // The attacker never signed; the call must fail authorization.
    assert!(
        result.is_err(),
        "unauthorized caller executed a proposal"
    );

    // Sanity: the proposal is still pending, not executed.
    assert_ne!(client.get_state(&proposal), ProposalState::Executed);
    let _ = attacker;
}

/// Only the admin may schedule proposals; a random caller must be rejected.
#[test]
fn non_admin_cannot_schedule() {
    let (env, client, _admin, _executor) = setup();

    env.mock_auths(&[]);

    let proposal = id(&env, 30);
    let result = client
        .mock_auths(&[])
        .try_schedule(&proposal, &None, &(START_TS + 100));

    assert!(result.is_err(), "unauthorized caller scheduled a proposal");
    assert_eq!(client.get_state(&proposal), ProposalState::Unknown);
}

/// A proposal cannot be executed twice.
#[test]
fn double_execution_is_rejected() {
    let (env, client, _admin, _executor) = setup();

    let proposal = id(&env, 40);
    client.schedule(&proposal, &None, &(START_TS + 100));

    env.ledger().set(LedgerInfo {
        timestamp: START_TS + 1_000,
        protocol_version: 20,
        sequence_number: 2,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 16,
        max_entry_ttl: 6_312_000,
    });

    client.execute(&proposal);
    assert_eq!(client.get_state(&proposal), ProposalState::Executed);

    let result = client.try_execute(&proposal);
    assert!(result.is_err(), "proposal executed twice");
}

/// A chain of proposals must execute strictly in order; attempting to skip
/// ahead to a later proposal must fail.
#[test]
fn cannot_skip_predecessor_chain() {
    let (env, client, _admin, _executor) = setup();

    let a = id(&env, 50);
    let b = id(&env, 51);
    let c = id(&env, 52);

    client.schedule(&a, &None, &(START_TS + 100));
    client.schedule(&b, &Some(a.clone()), &(START_TS + 100));
    client.schedule(&c, &Some(b.clone()), &(START_TS + 100));

    env.ledger().set(LedgerInfo {
        timestamp: START_TS + 1_000,
        protocol_version: 20,
        sequence_number: 2,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 16,
        max_entry_ttl: 6_312_000,
    });

    // Attempting to execute the tail of the chain first must fail.
    assert!(
        client.try_execute(&c).is_err(),
        "tail of chain executed before predecessors"
    );
    assert!(
        client.try_execute(&b).is_err(),
        "middle of chain executed before predecessor"
    );

    // Executing in order succeeds.
    client.execute(&a);
    client.execute(&b);
    client.execute(&c);

    assert_eq!(client.get_state(&a), ProposalState::Executed);
    assert_eq!(client.get_state(&b), ProposalState::Executed);
    assert_eq!(client.get_state(&c), ProposalState::Executed);
}

/// Executing a proposal that was never scheduled must fail.
#[test]
fn executing_unknown_proposal_is_rejected() {
    let (env, client, _admin, _executor) = setup();

    let ghost = id(&env, 60);
    let result = client.try_execute(&ghost);
    assert!(result.is_err(), "executed an unscheduled proposal");
    assert_eq!(client.get_state(&ghost), ProposalState::Unknown);
}

/// Scheduling the same proposal twice must be rejected (replay protection).
#[test]
fn duplicate_schedule_is_rejected() {
    let (env, client, _admin, _executor) = setup();

    let proposal = id(&env, 70);
    client.schedule(&proposal, &None, &(START_TS + 100));

    let result = client.try_schedule(&proposal, &None, &(START_TS + 100));
    assert!(result.is_err(), "duplicate schedule accepted");
}

/// A proposal whose predecessor was cancelled must not be executable.
#[test]
fn cancelled_predecessor_blocks_dependent() {
    let (env, client, _admin, _executor) = setup();

    let first = id(&env, 80);
    let second = id(&env, 81);

    client.schedule(&first, &None, &(START_TS + 100));
    client.schedule(&second, &Some(first.clone()), &(START_TS + 100));

    client.cancel(&first);
    assert_eq!(client.get_state(&first), ProposalState::Cancelled);

    env.ledger().set(LedgerInfo {
        timestamp: START_TS + 1_000,
        protocol_version: 20,
        sequence_number: 2,
        network_id: Default::default(),
        base_reserve: 10,
        min_temp_entry_ttl: 16,
        min_persistent_entry_ttl: 16,
        max_entry_ttl: 6_312_000,
    });

    let result = client.try_execute(&second);
    assert!(
        result.is_err(),
        "dependent proposal executed after predecessor was cancelled"
    );
}

/// Helper to keep the `Vec` import meaningful for future multi-predecessor
/// scenarios without triggering unused-import warnings today.
#[allow(dead_code)]
fn _unused_vec_marker(env: &Env) -> Vec<BytesN<32>> {
    Vec::new(env)
}