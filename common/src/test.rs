#![cfg(test)]

//! Upgrade-compatibility harness for the migration framework: a
//! throwaway contract (test builds only) whose "new wasm" has two
//! registered steps, one of them paginated, run against a contract left
//! at schema version 1.

use crate::migrations::{self, MigrationErrors, Step, BASELINE_SCHEMA_VERSION};
use crate::testutils::{
    advance_ledgers, advance_seconds, advance_time, assert_expired, assert_not_expired,
    assert_stale, set_ledger_time, LEDGERS_PER_SECOND,
};
use soroban_sdk::{contract, contracterror, contractimpl, symbol_short, Env};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum HarnessError {
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
}

const ERRORS: MigrationErrors<HarnessError> = MigrationErrors {
    version_mismatch: HarnessError::SchemaVersionMismatch,
    invalid_target: HarnessError::InvalidMigrationTarget,
    in_progress: HarnessError::MigrationInProgress,
    not_in_progress: HarnessError::NoMigrationInProgress,
    invalid_batch: HarnessError::InvalidMigrationBatch,
};

const CURRENT: u32 = 3;
/// Items step 3 rewrites — more than one default batch.
const ITEMS: u32 = 120;

fn step(env: &Env, to: u32, cursor: u32, limit: u32) -> Step {
    match to {
        2 => {
            env.storage().instance().set(&symbol_short!("v2"), &true);
            Step::Done
        }
        3 => {
            let end = cursor.saturating_add(limit).min(ITEMS);
            let done: u32 = env
                .storage()
                .instance()
                .get(&symbol_short!("v3"))
                .unwrap_or(0);
            env.storage()
                .instance()
                .set(&symbol_short!("v3"), &(done + end - cursor));
            if end == ITEMS {
                Step::Done
            } else {
                Step::More(end)
            }
        }
        _ => panic!("no step"),
    }
}

#[contract]
pub struct Harness;

#[contractimpl]
impl Harness {
    pub fn init(env: Env, version: u32) {
        migrations::init_schema(&env, version);
    }

    pub fn migrate(env: Env, from: u32, to: u32) -> u32 {
        migrations::migrate(&env, from, to, CURRENT, step, &ERRORS)
    }

    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> u32 {
        migrations::migrate_batch(&env, cursor, limit, step, &ERRORS)
    }

    pub fn version(env: Env) -> u32 {
        migrations::schema_version(&env)
    }

    pub fn state(env: Env) -> Option<(u32, u32)> {
        migrations::migration_state(&env)
    }

    pub fn guarded(env: Env) {
        migrations::require_schema_current(&env, CURRENT, HarnessError::MigrationInProgress);
    }

    pub fn processed(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("v3"))
            .unwrap_or(0)
    }
}

fn setup<'a>() -> (Env, HarnessClient<'a>) {
    let env = Env::default();
    let id = env.register_contract(None, Harness);
    let client = HarnessClient::new(&env, &id);
    (env, client)
}

#[test]
fn missing_version_reads_as_baseline() {
    let (_, client) = setup();
    assert_eq!(client.version(), BASELINE_SCHEMA_VERSION);
}

#[test]
fn stale_schema_blocks_guarded_entrypoints() {
    let (_, client) = setup();
    client.init(&1);
    assert_eq!(
        client.try_guarded(),
        Err(Ok(HarnessError::MigrationInProgress))
    );
}

#[test]
fn paginated_migration_runs_every_step_once_in_order() {
    let (_, client) = setup();
    client.init(&1);

    // Step 2 completes; step 3 stops after one default batch.
    assert_eq!(client.migrate(&1, &3), 2);
    assert_eq!(
        client.state(),
        Some((3, migrations::DEFAULT_MIGRATION_BATCH))
    );
    assert_eq!(client.processed(), migrations::DEFAULT_MIGRATION_BATCH);
    assert_eq!(
        client.try_guarded(),
        Err(Ok(HarnessError::MigrationInProgress))
    );

    // `migrate` can't be re-entered while a batch is in flight.
    assert_eq!(
        client.try_migrate(&2, &3),
        Err(Ok(HarnessError::MigrationInProgress))
    );
    // Replaying or skipping pages is rejected, as are bad limits.
    assert_eq!(
        client.try_migrate_batch(&0, &50),
        Err(Ok(HarnessError::InvalidMigrationBatch))
    );
    assert_eq!(
        client.try_migrate_batch(&100, &50),
        Err(Ok(HarnessError::InvalidMigrationBatch))
    );
    assert_eq!(
        client.try_migrate_batch(&50, &0),
        Err(Ok(HarnessError::InvalidMigrationBatch))
    );
    assert_eq!(
        client.try_migrate_batch(&50, &(migrations::MAX_MIGRATION_BATCH + 1)),
        Err(Ok(HarnessError::InvalidMigrationBatch))
    );

    assert_eq!(client.migrate_batch(&50, &40), 2);
    assert_eq!(client.state(), Some((3, 90)));
    assert_eq!(client.migrate_batch(&90, &40), 3);
    assert_eq!(client.state(), None);
    assert_eq!(client.processed(), ITEMS);
    client.guarded();

    assert_eq!(
        client.try_migrate_batch(&0, &50),
        Err(Ok(HarnessError::NoMigrationInProgress))
    );
}

#[test]
fn rejects_repeating_or_skipping_steps() {
    let (_, client) = setup();
    client.init(&1);

    // `from` must be the stored version.
    assert_eq!(
        client.try_migrate(&2, &3),
        Err(Ok(HarnessError::SchemaVersionMismatch))
    );
    // Target must be ahead of `from` and known to this wasm.
    assert_eq!(
        client.try_migrate(&1, &1),
        Err(Ok(HarnessError::InvalidMigrationTarget))
    );
    assert_eq!(
        client.try_migrate(&1, &4),
        Err(Ok(HarnessError::InvalidMigrationTarget))
    );

    assert_eq!(client.migrate(&1, &2), 2);
    // Step 2 already ran: repeating it fails.
    assert_eq!(
        client.try_migrate(&1, &2),
        Err(Ok(HarnessError::SchemaVersionMismatch))
    );
}

// ---------------------------------------------------------------------------
// Time-travel helper tests
// ---------------------------------------------------------------------------

#[test]
fn set_ledger_time_sets_absolute_timestamp_and_sequence() {
    let env = Env::default();
    set_ledger_time(&env, 1_000, 42);
    assert_eq!(env.ledger().timestamp(), 1_000);
    assert_eq!(env.ledger().sequence(), 42);

    // Setting a later absolute time is allowed.
    set_ledger_time(&env, 5_000, 100);
    assert_eq!(env.ledger().timestamp(), 5_000);
    assert_eq!(env.ledger().sequence(), 100);
}

#[test]
fn advance_seconds_moves_timestamp_only() {
    let env = Env::default();
    set_ledger_time(&env, 1_000, 10);
    advance_seconds(&env, 250);
    assert_eq!(env.ledger().timestamp(), 1_250);
    assert_eq!(env.ledger().sequence(), 10);
}

#[test]
fn advance_ledgers_moves_sequence_only() {
    let env = Env::default();
    set_ledger_time(&env, 1_000, 10);
    advance_ledgers(&env, 7);
    assert_eq!(env.ledger().sequence(), 17);
    assert_eq!(env.ledger().timestamp(), 1_000);
}

#[test]
fn advance_time_moves_both_in_documented_ratio() {
    let env = Env::default();
    set_ledger_time(&env, 1_000, 10);
    advance_time(&env, 60);
    assert_eq!(env.ledger().timestamp(), 1_060);
    assert_eq!(env.ledger().sequence(), 10 + 60 * LEDGERS_PER_SECOND);
}

#[test]
fn advance_time_rejects_zero_delta() {
    let env = Env::default();
    set_ledger_time(&env, 1_000, 10);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        advance_time(&env, 0);
    }));
    assert!(result.is_err());
}

#[test]
#[should_panic]
fn set_ledger_time_rejects_moving_backwards() {
    let env = Env::default();
    set_ledger_time(&env, 1_000, 10);
    set_ledger_time(&env, 999, 10);
}

#[test]
#[should_panic]
fn advance_seconds_rejects_negative_delta() {
    let env = Env::default();
    set_ledger_time(&env, 1_000, 10);
    advance_seconds(&env, -1);
}

#[test]
#[should_panic]
fn advance_ledgers_rejects_negative_delta() {
    let env = Env::default();
    set_ledger_time(&env, 1_000, 10);
    advance_ledgers(&env, -1);
}

#[test]
fn expiry_assertions_track_boundary() {
    let env = Env::default();
    set_ledger_time(&env, 1_000, 10);
    let deadline = 1_500;

    assert_not_expired(&env, deadline);
    advance_seconds(&env, 499);
    assert_not_expired(&env, deadline);
    advance_seconds(&env, 1);
    assert_expired(&env, deadline);
}

#[test]
fn staleness_assertions_track_boundary() {
    let env = Env::default();
    set_ledger_time(&env, 1_000, 10);
    let updated_at = 1_000;
    let max_age = 300;

    assert_stale(&env, updated_at, max_age);
    advance_seconds(&env, 299);
    assert_stale(&env, updated_at, max_age);
    advance_seconds(&env, 1);
    // Exactly at max_age the report is still fresh.
    assert!(!crate::testutils::is_stale(&env, updated_at, max_age));
    advance_seconds(&env, 1);
    assert!(!crate::testutils::is_stale(&env, updated_at, max_age));
}
