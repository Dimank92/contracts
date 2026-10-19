#![cfg(test)]

//! Upgrade-compatibility harness for the migration and upgrade-safety
//! frameworks: a throwaway contract (test builds only) whose "new wasm"
//! has two registered steps, one of them paginated, run against a
//! contract left at schema version 1. The allowlist and post-upgrade
//! check are exercised here up to the wasm swap; multisig's
//! `upgrade-wasm` tests run the full install against a real wasm.

use crate::events::{UpgradeCheckFailed, UpgradeChecked, UpgradeProposed, EVENTS_VERSION};
use crate::migrations::{self, MigrationErrors, Step, BASELINE_SCHEMA_VERSION};
use crate::upgrade::{self, PendingCheck, UpgradeErrors, UPGRADE_DELAY};
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short,
    testutils::{Events as _, Ledger as _},
    BytesN, Env, IntoVal, Symbol, TryFromVal, Val, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum HarnessError {
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
    WasmNotProposed = 105,
    UpgradeNotMatured = 106,
    WasmAlreadyProposed = 107,
    UpgradeCheckPending = 108,
    NoUpgradeCheckPending = 109,
    Unauthorized = 110,
    NotInitialized = 111,
    BrokenInvariant = 112,
}

const ERRORS: MigrationErrors<HarnessError> = MigrationErrors {
    version_mismatch: HarnessError::SchemaVersionMismatch,
    invalid_target: HarnessError::InvalidMigrationTarget,
    in_progress: HarnessError::MigrationInProgress,
    not_in_progress: HarnessError::NoMigrationInProgress,
    invalid_batch: HarnessError::InvalidMigrationBatch,
};

const UPGRADE_ERRORS: UpgradeErrors<HarnessError> = UpgradeErrors {
    unauthorized: HarnessError::Unauthorized,
    not_initialized: HarnessError::NotInitialized,
    not_proposed: HarnessError::WasmNotProposed,
    not_matured: HarnessError::UpgradeNotMatured,
    already_proposed: HarnessError::WasmAlreadyProposed,
    check_pending: HarnessError::UpgradeCheckPending,
    no_check_pending: HarnessError::NoUpgradeCheckPending,
    migration_in_progress: HarnessError::MigrationInProgress,
};

/// Stands in for the private guard key: what `install` writes, minus the
/// wasm swap (which needs real wasm; see multisig's upgrade-wasm tests).
#[contracttype]
enum TestKey {
    Broken,
}

const CURRENT: u32 = 3;
/// Items step 3 rewrites — more than one default batch.
const ITEMS: u32 = 120;

fn step(env: &Env, to: u32, cursor: u32, limit: u32) -> Result<Step, HarnessError> {
    Ok(match to {
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
        _ => return Err(HarnessError::InvalidMigrationTarget),
    })
}

/// The harness's post-upgrade invariant: fails once `break_invariant` ran.
fn invariants(env: &Env) -> Result<(), HarnessError> {
    if env.storage().instance().has(&TestKey::Broken) {
        return Err(HarnessError::BrokenInvariant);
    }
    Ok(())
}

#[contract]
pub struct Harness;

#[contractimpl]
impl Harness {
    pub fn init(env: Env, version: u32) {
        migrations::init_schema(&env, version);
    }

    pub fn migrate(env: Env, from: u32, to: u32) -> Result<u32, HarnessError> {
        migrations::migrate(&env, from, to, CURRENT, step, &ERRORS)
    }

    pub fn migrate_batch(env: Env, cursor: u32, limit: u32) -> Result<u32, HarnessError> {
        migrations::migrate_batch(&env, cursor, limit, step, &ERRORS)
    }

    pub fn version(env: Env) -> u32 {
        migrations::schema_version(&env)
    }

    pub fn state(env: Env) -> Option<(u32, u32)> {
        migrations::migration_state(&env)
    }

    /// A user-facing entrypoint: closed during migrations and until the
    /// post-upgrade check passes.
    pub fn guarded(env: Env) -> Result<(), HarnessError> {
        migrations::require_schema_current(&env, CURRENT, HarnessError::MigrationInProgress)?;
        upgrade::require_checked(&env, HarnessError::UpgradeCheckPending)
    }

    pub fn propose_wasm(env: Env, hash: BytesN<32>) -> Result<u64, HarnessError> {
        upgrade::propose_wasm(&env, &hash, UPGRADE_DELAY, false, &UPGRADE_ERRORS)
    }

    pub fn cancel_wasm(env: Env, hash: BytesN<32>) -> Result<(), HarnessError> {
        upgrade::cancel_wasm(&env, &hash, &UPGRADE_ERRORS)
    }

    pub fn proposed(env: Env, hash: BytesN<32>) -> Option<u64> {
        upgrade::proposed_wasm(&env, &hash).map(|p| p.not_before)
    }

    /// `upgrade::install` against a hash that was never uploaded: every
    /// check runs, and only a passing one reaches the (failing) swap.
    pub fn install(env: Env, hash: BytesN<32>) -> Result<(), HarnessError> {
        upgrade::install(&env, &hash, &UPGRADE_ERRORS)
    }

    pub fn post_upgrade_check(env: Env) -> Result<bool, HarnessError> {
        upgrade::post_upgrade_check(&env, CURRENT, invariants, &UPGRADE_ERRORS)
    }

    pub fn pending_check(env: Env) -> Option<PendingCheck> {
        upgrade::pending_check(&env)
    }

    pub fn pin(env: Env, multisig: soroban_sdk::Address) {
        upgrade::pin_multisig(&env, &multisig);
    }

    pub fn break_invariant(env: Env) {
        env.storage().instance().set(&TestKey::Broken, &true);
    }

    pub fn fix_invariant(env: Env) {
        env.storage().instance().remove(&TestKey::Broken);
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

// ── Upgrade safety (#127) ────────────────────────────────────────────────────

fn hash(env: &Env, byte: u8) -> BytesN<32> {
    BytesN::from_array(env, &[byte; 32])
}

/// Raises the post-upgrade guard the way `install` does, for `hash`.
fn raise_guard(env: &Env, client: &HarnessClient, hash: &BytesN<32>) {
    env.as_contract(&client.address, || {
        // Same encoding as zenith_common::upgrade's private key.
        #[contracttype]
        enum UpgradeKey {
            PendingCheck,
        }
        env.storage().instance().set(
            &UpgradeKey::PendingCheck,
            &PendingCheck {
                wasm_hash: hash.clone(),
                installed_at: env.ledger().timestamp(),
                failed: false,
                reason: 0,
            },
        );
    });
}

fn last_event(env: &Env) -> (Vec<Val>, Val) {
    let (_, topics, data) = env.events().all().last().unwrap();
    (topics, data)
}

#[test]
fn proposing_allowlists_with_a_delay_and_emits() {
    let (env, client) = setup();
    client.init(&CURRENT);
    env.ledger().set_timestamp(1_000);
    let h = hash(&env, 1);

    let not_before = client.propose_wasm(&h);
    assert_eq!(not_before, 1_000 + UPGRADE_DELAY);
    assert_eq!(client.proposed(&h), Some(not_before));

    let (topics, data) = last_event(&env);
    assert_eq!(
        topics,
        (
            Symbol::new(&env, "upgrade_proposed"),
            EVENTS_VERSION,
            h.clone()
        )
            .into_val(&env)
    );
    let data = UpgradeProposed::try_from_val(&env, &data).unwrap();
    assert_eq!(
        data,
        UpgradeProposed {
            wasm_hash: h.clone(),
            not_before,
            emergency: false,
        }
    );

    assert_eq!(
        client.try_propose_wasm(&h),
        Err(Ok(HarnessError::WasmAlreadyProposed))
    );
}

#[test]
fn install_rejects_unlisted_and_immature_hashes() {
    let (env, client) = setup();
    client.init(&CURRENT);
    env.ledger().set_timestamp(1_000);
    let h = hash(&env, 1);

    assert_eq!(
        client.try_install(&h),
        Err(Ok(HarnessError::WasmNotProposed))
    );
    client.propose_wasm(&h);
    env.ledger().set_timestamp(1_000 + UPGRADE_DELAY - 1);
    assert_eq!(
        client.try_install(&h),
        Err(Ok(HarnessError::UpgradeNotMatured))
    );
    // Matured: every check passes and only the swap of a never-uploaded
    // hash fails (a host error, not a typed one), reverting everything.
    env.ledger().set_timestamp(1_000 + UPGRADE_DELAY);
    assert!(matches!(client.try_install(&h), Err(Err(_))));
    assert_eq!(client.proposed(&h), Some(1_000 + UPGRADE_DELAY));
    assert_eq!(client.pending_check(), None);
}

#[test]
fn cancelled_hash_can_no_longer_be_installed() {
    let (env, client) = setup();
    client.init(&CURRENT);
    let h = hash(&env, 1);
    client.propose_wasm(&h);
    client.cancel_wasm(&h);
    assert_eq!(client.proposed(&h), None);
    env.ledger().set_timestamp(UPGRADE_DELAY);
    assert_eq!(
        client.try_install(&h),
        Err(Ok(HarnessError::WasmNotProposed))
    );
    assert_eq!(
        client.try_cancel_wasm(&h),
        Err(Ok(HarnessError::WasmNotProposed))
    );
}

#[test]
fn guard_blocks_entrypoints_until_the_check_passes() {
    let (env, client) = setup();
    client.init(&CURRENT);
    assert_eq!(
        client.try_post_upgrade_check(),
        Err(Ok(HarnessError::NoUpgradeCheckPending))
    );
    let h = hash(&env, 2);
    raise_guard(&env, &client, &h);
    assert_eq!(
        client.try_guarded(),
        Err(Ok(HarnessError::UpgradeCheckPending))
    );

    assert!(client.post_upgrade_check());
    assert_eq!(client.pending_check(), None);
    client.guarded();
    let (topics, data) = last_event(&env);
    assert_eq!(
        topics,
        (
            Symbol::new(&env, "upgrade_checked"),
            EVENTS_VERSION,
            h.clone()
        )
            .into_val(&env)
    );
    assert_eq!(
        UpgradeChecked::try_from_val(&env, &data).unwrap(),
        UpgradeChecked {
            wasm_hash: h,
            schema_version: CURRENT,
        }
    );
}

#[test]
fn check_waits_for_migrate() {
    let (env, client) = setup();
    client.init(&1);
    raise_guard(&env, &client, &hash(&env, 2));
    // Schema lags the code: nothing is recorded, run migrate first.
    assert_eq!(
        client.try_post_upgrade_check(),
        Err(Ok(HarnessError::MigrationInProgress))
    );
    client.migrate(&1, &3);
    client.migrate_batch(&50, &200);
    assert_eq!(client.version(), CURRENT);
    assert!(client.post_upgrade_check());
    client.guarded();
}

#[test]
fn failing_check_records_the_reason_and_stays_closed() {
    let (env, client) = setup();
    client.init(&CURRENT);
    let h = hash(&env, 3);
    raise_guard(&env, &client, &h);
    client.break_invariant();

    assert!(!client.post_upgrade_check());
    let pending = client.pending_check().unwrap();
    assert!(pending.failed);
    assert_eq!(pending.reason, HarnessError::BrokenInvariant as u32);
    assert_eq!(
        client.try_guarded(),
        Err(Ok(HarnessError::UpgradeCheckPending))
    );
    let (topics, data) = last_event(&env);
    assert_eq!(
        topics,
        (
            Symbol::new(&env, "upgrade_check_failed"),
            EVENTS_VERSION,
            h.clone()
        )
            .into_val(&env)
    );
    assert_eq!(
        UpgradeCheckFailed::try_from_val(&env, &data).unwrap(),
        UpgradeCheckFailed {
            wasm_hash: h,
            reason: HarnessError::BrokenInvariant as u32,
        }
    );

    // A fixed state (in practice: a fixed wasm) passes a later check.
    client.fix_invariant();
    assert!(client.post_upgrade_check());
    client.guarded();
}

#[test]
fn shared_events_conform_to_the_spec() {
    use soroban_sdk::testutils::Address as _;

    let (env, client) = setup();
    let mut rec = event_spec::Recorder::shared(&client.address);
    client.init(&1);
    client.migrate(&1, &3);
    client.migrate_batch(&50, &200);
    client.pin(&soroban_sdk::Address::generate(&env));
    let h = hash(&env, 1);
    client.propose_wasm(&h);
    client.cancel_wasm(&h);
    raise_guard(&env, &client, &h);
    client.break_invariant();
    client.post_upgrade_check();
    client.fix_invariant();
    client.post_upgrade_check();
    rec.record(&env);
    // `upgraded` needs a real wasm swap: multisig's upgrade-wasm tests
    // check it against the spec.
    rec.assert_complete_except(&["upgraded"]);
}
