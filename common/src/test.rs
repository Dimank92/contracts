#![cfg(test)]

//! Upgrade-compatibility harness for the migration framework: a
//! throwaway contract (test builds only) whose "new wasm" has two
//! registered steps, one of them paginated, run against a contract left
//! at schema version 1.

use crate::migrations::{self, MigrationErrors, Step, BASELINE_SCHEMA_VERSION};
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

    pub fn migrate(env: Env, from: u32, to: u32) -> Result<u32, HarnessError> {
        Ok(migrations::migrate(&env, from, to, CURRENT, step, &ERRORS))
    }

    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> Result<u32, HarnessError> {
        Ok(migrations::migrate_batch(
            &env, cursor, limit, step, &ERRORS,
        ))
    }

    pub fn version(env: Env) -> u32 {
        migrations::schema_version(&env)
    }

    pub fn state(env: Env) -> Option<(u32, u32)> {
        migrations::migration_state(&env)
    }

    pub fn guarded(env: Env) -> Result<(), HarnessError> {
        migrations::require_schema_current(&env, CURRENT, HarnessError::MigrationInProgress);
        Ok(())
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
