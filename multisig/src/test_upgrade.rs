#![cfg(all(test, feature = "upgrade-wasm"))]

//! Upgrade safety (#127) end to end, against this crate's own release
//! wasm: `cargo xtask test -p multisig` builds it (`self-wasm`) and runs
//! this module (`--features upgrade-wasm`).

extern crate std;

use crate::{DataKey, Error, MultisigClient, UPGRADE_DELAY};
use event_spec::{events_named, Recorder};
use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, vec, Address, BytesN, Env};
use zenith_common::events::{UpgradeCheckFailed, UpgradeChecked, UpgradeProposed, Upgraded};

const WASM: &[u8] =
    include_bytes!("../../target/wasm32-unknown-unknown/release/zenith_multisig.wasm");

struct Harness<'a> {
    env: Env,
    client: MultisigClient<'a>,
    a: Address,
    b: Address,
    wasm_hash: BytesN<32>,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    // Uploading and instantiating a real wasm exceeds the default test
    // budget; resource costs are measured elsewhere.
    env.budget().reset_unlimited();
    env.ledger().set_timestamp(1_000);
    let id = env.register_contract_wasm(None, WASM);
    let client = MultisigClient::new(&env, &id);
    let (a, b) = (Address::generate(&env), Address::generate(&env));
    client.initialize(&vec![&env, a.clone(), b.clone()], &1, &0);
    // Any uploaded wasm can be the "new" code; reuse this one.
    let wasm_hash = env.deployer().upload_contract_wasm(WASM);
    Harness {
        env,
        client,
        a,
        b,
        wasm_hash,
    }
}

impl Harness<'_> {
    /// Unanimity, then the delay: `upgrade(wasm_hash)` becomes callable.
    fn allowlist_and_mature(&self) -> u64 {
        self.client.approve_upgrade(&self.a, &self.wasm_hash);
        self.client.approve_upgrade(&self.b, &self.wasm_hash);
        let not_before = self.client.get_upgrade_ready_at(&self.wasm_hash).unwrap();
        self.env.ledger().set_timestamp(not_before);
        not_before
    }

    fn approve_is_open(&self, n: u8) -> bool {
        match self
            .client
            .try_approve(&self.a, &BytesN::from_array(&self.env, &[n; 32]))
        {
            Ok(_) => true,
            Err(Ok(Error::UpgradeCheckPending)) => false,
            other => panic!("unexpected approve result: {other:?}"),
        }
    }
}

#[test]
fn upgrade_needs_unanimity_and_a_matured_allowlist_entry() {
    let h = setup();
    h.client.approve_upgrade(&h.a, &h.wasm_hash);
    assert_eq!(h.client.get_upgrade_ready_at(&h.wasm_hash), None);
    assert_eq!(
        h.client.try_upgrade(&h.wasm_hash),
        Err(Ok(Error::UpgradeNotUnanimous))
    );

    h.client.approve_upgrade(&h.b, &h.wasm_hash);
    let not_before = h.client.get_upgrade_ready_at(&h.wasm_hash).unwrap();
    assert_eq!(not_before, 1_000 + UPGRADE_DELAY);
    assert_eq!(
        events_named::<UpgradeProposed>(&h.env, &h.client.address, "upgrade_proposed"),
        [UpgradeProposed {
            wasm_hash: h.wasm_hash.clone(),
            not_before,
            emergency: false,
        }]
    );

    // Before maturity: rejected.
    h.env.ledger().set_timestamp(not_before - 1);
    assert_eq!(
        h.client.try_upgrade(&h.wasm_hash),
        Err(Ok(Error::UpgradeNotMatured))
    );

    // A revoke drops it from the allowlist; unanimity again restarts it.
    h.client.revoke_upgrade(&h.b, &h.wasm_hash);
    assert_eq!(h.client.get_upgrade_ready_at(&h.wasm_hash), None);
    h.client.approve_upgrade(&h.b, &h.wasm_hash);
    assert_eq!(
        h.client.get_upgrade_ready_at(&h.wasm_hash),
        Some(not_before - 1 + UPGRADE_DELAY)
    );
}

#[test]
fn matured_upgrade_installs_and_stays_closed_until_checked() {
    let h = setup();
    let mut rec = Recorder::new("multisig", &h.client.address);
    h.allowlist_and_mature();

    h.client.upgrade(&h.wasm_hash);
    rec.record(&h.env);
    assert_eq!(
        events_named::<Upgraded>(&h.env, &h.client.address, "upgraded"),
        [Upgraded {
            wasm_hash: h.wasm_hash.clone(),
            schema_version: 1,
        }]
    );
    // The allowlist entry is consumed: the same approval can't reinstall.
    assert_eq!(h.client.get_upgrade_ready_at(&h.wasm_hash), None);
    let status = h.client.get_upgrade_status().unwrap();
    assert_eq!(
        (status.wasm_hash.clone(), status.failed),
        (h.wasm_hash.clone(), false)
    );

    // User-facing entrypoints are closed; views and the check aren't.
    assert!(!h.approve_is_open(1));
    assert_eq!(h.client.get_threshold(), 1);

    assert!(h.client.post_upgrade_check());
    rec.record(&h.env);
    assert_eq!(
        events_named::<UpgradeChecked>(&h.env, &h.client.address, "upgrade_checked"),
        [UpgradeChecked {
            wasm_hash: h.wasm_hash.clone(),
            schema_version: 1,
        }]
    );
    assert_eq!(h.client.get_upgrade_status(), None);
    assert!(h.approve_is_open(1));
    assert_eq!(
        h.client.try_post_upgrade_check(),
        Err(Ok(Error::NoUpgradeCheckPending))
    );
}

#[test]
fn failing_post_check_keeps_the_contract_guarded() {
    let h = setup();
    let mut rec = Recorder::new("multisig", &h.client.address);
    h.allowlist_and_mature();
    h.client.upgrade(&h.wasm_hash);

    // Simulate a botched upgrade that left the config invalid.
    h.env.as_contract(&h.client.address, || {
        h.env.storage().instance().set(&DataKey::Threshold, &0u32);
    });
    assert!(!h.client.post_upgrade_check());
    rec.record(&h.env);
    let status = h.client.get_upgrade_status().unwrap();
    assert!(status.failed);
    assert_eq!(status.reason, Error::InvalidThreshold as u32);
    assert_eq!(
        events_named::<UpgradeCheckFailed>(&h.env, &h.client.address, "upgrade_check_failed"),
        [UpgradeCheckFailed {
            wasm_hash: h.wasm_hash.clone(),
            reason: Error::InvalidThreshold as u32,
        }]
    );
    assert!(!h.approve_is_open(1));

    // Still guarded on a retry; a fix (in practice, a fixed wasm going
    // through the same allowlist) lets a later check pass.
    assert!(!h.client.post_upgrade_check());
    assert!(!h.approve_is_open(1));
    h.env.as_contract(&h.client.address, || {
        h.env.storage().instance().set(&DataKey::Threshold, &1u32);
    });
    assert!(h.client.post_upgrade_check());
    assert!(h.approve_is_open(1));
}
