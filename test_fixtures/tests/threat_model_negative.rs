#![cfg(test)]

//! # THREAT_MODEL-Driven Negative Test Suite for Auth and Admin Bypass (Issue #198)
//!
//! This suite walks `THREAT_MODEL.md` line by line and asserts concrete, typed
//! rejections against live contract instances for identified authorization
//! and admin-bypass threat paths across the Zenith protocol contracts:
//!
//! - Section 1: Trust Boundaries & Privileged Actors (Admin, Multisig, Feeders, Timelock)
//! - Section 2.1: Rate-Limited Outflows & Circuit Breakers (Untested / Tracked follow-up #19)
//! - Section 2.2: Protocol Guardian & Auto-Expiring Pauses (Untested / Tracked follow-up #18)
//! - Section 2.3: Granular Access Control Roles (Untested / Tracked follow-up #17)
//! - Section 2.4: Flash-Loan / Oracle Price Manipulation & Feeder Consensus
//! - Section 2.5: Solvency Invariant & Cross-Tag Collateral Isolation in Vault
//! - Section 2.6: Multisig Quorum & Timelock Tiers (Tracked follow-up #150)

extern crate std;

use multisig::{Multisig, MultisigClient, SignerWeight};
use price_oracle::{PriceOracle, PriceOracleClient};
use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, Ledger as _},
    vec, Address, BytesN, Env, IntoVal, Symbol, Vec,
};
use test_adversaries::tokens::FeeOnTransferToken;
use timelock::{Call, Timelock, TimelockClient};
use vault::{Tag, Vault, VaultClient};

fn contract_err(code: u32) -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(code)
}

// Stand-in target contract for timelock tests.
#[contract]
struct MockAdminTarget;

#[contractimpl]
impl MockAdminTarget {
    pub fn execute_action(env: Env, admin: Address, val: u32) {
        admin.require_auth();
        env.storage().instance().set(&0u32, &val);
    }

    pub fn get_val(env: Env) -> u32 {
        env.storage().instance().get(&0u32).unwrap_or(0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 1 & Core Requirement 1: Non-Admin Admin Entrypoint Bypass Rejections
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn threat_1_vault_unauthorized_non_admin_cannot_transfer_admin() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin).address();
    let vault_id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &vault_id);

    // Initialize with admin auth.
    env.mock_all_auths();
    client.initialize(&admin, &token);

    // Clear mock auths: non-admin call must fail authorization.
    env.mock_auths(&[]);
    let non_admin = Address::generate(&env);
    let res = client.try_transfer_admin(&non_admin);
    assert!(res.is_err(), "transfer_admin must reject non-admin call");
}

#[test]
fn threat_1_vault_unapproved_multisig_admin_actions_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin).address();
    let vault_id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &vault_id);
    client.initialize(&admin, &token);

    let ms_id = env.register_contract(None, Multisig);
    let ms_client = MultisigClient::new(&env, &ms_id);
    let signers = vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    ms_client.initialize(&signers, &2, &0);

    let unapproved_action = BytesN::from_array(&env, &[99u8; 32]);
    let new_admin = Address::generate(&env);

    // transfer_admin_via_multisig with unapproved action fails Unauthorized (error #6)
    let res = client.try_transfer_admin_via_multisig(&ms_id, &unapproved_action, &new_admin);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(vault::Error::Unauthorized as u32))),
        "transfer_admin_via_multisig must reject unapproved action with Unauthorized"
    );

    // pause_via_multisig with unapproved action fails Unauthorized
    let res = client.try_pause_via_multisig(&ms_id, &unapproved_action);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(vault::Error::Unauthorized as u32))),
        "pause_via_multisig must reject unapproved action with Unauthorized"
    );

    // unpause_via_multisig with unapproved action fails Unauthorized
    let res = client.try_unpause_via_multisig(&ms_id, &unapproved_action);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(vault::Error::Unauthorized as u32))),
        "unpause_via_multisig must reject unapproved action with Unauthorized"
    );

    // sweep_untagged_via_multisig with unapproved action fails Unauthorized
    let res = client.try_sweep_untagged_via_multisig(&ms_id, &unapproved_action, &token, &new_admin);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(vault::Error::Unauthorized as u32))),
        "sweep_untagged_via_multisig must reject unapproved action with Unauthorized"
    );
}

#[test]
fn threat_1_price_oracle_unauthorized_non_admin_cannot_mutate_config() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let oracle_id = env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&env, &oracle_id);

    env.mock_all_auths();
    client.initialize(&admin);

    let stranger = Address::generate(&env);
    let ms_id = env.register_contract(None, Multisig);
    let unapproved_action = BytesN::from_array(&env, &[99u8; 32]);

    // Via multisig with unapproved action must reject with Unauthorized (error #9).
    let res = client.try_add_feeder_via_multisig(&ms_id, &unapproved_action, &stranger);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(price_oracle::Error::Unauthorized as u32))),
        "add_feeder_via_multisig must reject unapproved action"
    );

    let res = client.try_set_min_reports_via_multisig(&ms_id, &unapproved_action, &3);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(price_oracle::Error::Unauthorized as u32))),
        "set_min_reports_via_multisig must reject unapproved action"
    );

    let res = client.try_set_max_staleness_via_multisig(&ms_id, &unapproved_action, &7200);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(price_oracle::Error::Unauthorized as u32))),
        "set_max_staleness_via_multisig must reject unapproved action"
    );

    let res = client.try_transfer_admin_via_multisig(&ms_id, &unapproved_action, &stranger);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(price_oracle::Error::Unauthorized as u32))),
        "transfer_admin_via_multisig must reject unapproved action"
    );
}

#[test]
fn threat_1_timelock_unauthorized_roles_cannot_schedule_execute_or_cancel() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let proposer = Address::generate(&env);
    let executor = Address::generate(&env);
    let canceller = Address::generate(&env);
    let stranger = Address::generate(&env);

    let tl_id = env.register_contract(None, Timelock);
    let tl_client = TimelockClient::new(&env, &tl_id);

    let proposers = vec![&env, proposer.clone()];
    let executors = vec![&env, executor.clone()];
    let cancellers = vec![&env, canceller.clone()];
    tl_client.initialize(&86_400, &proposers, &executors, &cancellers);

    let target_id = env.register_contract(None, MockAdminTarget);
    let calls = vec![
        &env,
        Call {
            target: target_id,
            function: Symbol::new(&env, "execute_action"),
            args: vec![&env, admin.to_val(), 42u32.into_val(&env)],
        },
    ];
    let salt = BytesN::from_array(&env, &[1u8; 32]);

    // Stranger attempting to schedule -> Unauthorized (Error #2)
    let res = tl_client.try_schedule(&stranger, &calls, &None, &salt, &86_400);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(timelock::Error::Unauthorized as u32))),
        "Non-proposer scheduling must fail with Unauthorized"
    );

    // Valid schedule by proposer
    let op_id = tl_client.schedule(&proposer, &calls, &None, &salt, &86_400);
    let ready_at = env.ledger().timestamp() + 86_400;

    // Stranger attempting to execute -> Unauthorized (Error #2)
    env.ledger().set_timestamp(ready_at);
    let res = tl_client.try_execute(&stranger, &calls, &None, &salt);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(timelock::Error::Unauthorized as u32))),
        "Non-executor executing must fail with Unauthorized"
    );

    // Stranger attempting to cancel -> Unauthorized (Error #2)
    let res = tl_client.try_cancel(&stranger, &op_id);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(timelock::Error::Unauthorized as u32))),
        "Non-canceller cancelling must fail with Unauthorized"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 2.4 & Core Requirement 2: Price Oracle Consensus & Feeder Quorum
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn threat_2_single_feeder_cannot_determine_aggregate_alone() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let oracle_id = env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&env, &oracle_id);
    client.initialize(&admin);

    let f1 = Address::generate(&env);
    let f2 = Address::generate(&env);
    client.add_feeder(&f1);
    client.add_feeder(&f2);

    // Require 2 feeders for quorum.
    client.set_min_reports(&2);

    let xlm = Symbol::new(&env, "XLM");

    // Single feeder reports.
    client.report_price(&f1, &xlm, &1_200_000);

    // Aggregate MUST fail closed: None returned because quorum (2) is not met.
    assert_eq!(
        client.get_price(&xlm),
        None,
        "Single feeder must not determine aggregate when min_reports = 2"
    );

    // When second feeder reports, aggregate is determined.
    client.report_price(&f2, &xlm, &1_400_000);
    assert_eq!(
        client.get_price(&xlm),
        Some(1_300_000),
        "Two feeders satisfy quorum and produce median aggregate"
    );
}

#[test]
fn threat_2_unauthorized_feeder_cannot_report_price() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let oracle_id = env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&env, &oracle_id);
    client.initialize(&admin);

    let stranger = Address::generate(&env);
    let xlm = Symbol::new(&env, "XLM");

    // An unregistered address calling report_price -> NotAFeeder (Error #4)
    let res = client.try_report_price(&stranger, &xlm, &1_500_000);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(price_oracle::Error::NotAFeeder as u32))),
        "Unauthorized feeder reporting must reject with NotAFeeder"
    );
}

#[test]
fn threat_2_minority_outlier_feeder_cannot_skew_median_beyond_honest_bounds() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let oracle_id = env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&env, &oracle_id);
    client.initialize(&admin);

    let f1 = Address::generate(&env);
    let f2 = Address::generate(&env);
    let f3_adversary = Address::generate(&env);
    client.add_feeder(&f1);
    client.add_feeder(&f2);
    client.add_feeder(&f3_adversary);
    client.set_min_reports(&2);

    let xlm = Symbol::new(&env, "XLM");

    // Honest feeders report normal prices
    client.report_price(&f1, &xlm, &1_000_000);
    client.report_price(&f2, &xlm, &1_020_000);

    // Adversarial feeder tries extreme price manipulation (100x skew)
    client.report_price(&f3_adversary, &xlm, &100_000_000);

    // Median of [1_000_000, 1_020_000, 100_000_000] is 1_020_000
    // The aggregate is bounded by honest observations; the outlier cannot move it alone.
    let aggregate = client.get_price(&xlm).unwrap();
    assert_eq!(aggregate, 1_020_000);
    assert!(
        aggregate >= 1_000_000 && aggregate <= 1_020_000,
        "Outlier feeder must not dictate the median aggregate"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 2.5 & Core Requirement 3: Vault Solvency & Cross-Tag Collateral Isolation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn threat_3_cross_tag_withdrawal_cannot_drain_another_tags_deposit() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token = token_contract.address();
    let sac = soroban_sdk::token::StellarAssetClient::new(&env, &token);

    let vault_id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &vault_id);
    client.initialize(&admin, &token);

    let integrator = Address::generate(&env);
    client.set_integrator(&integrator, &true);

    let tag_a = Tag {
        owner: integrator.clone(),
        kind: Symbol::new(&env, "series"),
        id: 1,
    };
    let tag_b = Tag {
        owner: integrator.clone(),
        kind: Symbol::new(&env, "series"),
        id: 2,
    };

    let user_a = Address::generate(&env);
    let payee_b = Address::generate(&env);

    // User A funds Tag A with 1,000,000 tokens
    sac.mint(&user_a, &1_000_000);
    client.deposit(&user_a, &token, &tag_a, &1_000_000);

    // Tag B has 0 escrowed. Vault holds 1,000,000 raw token balance from Tag A.
    assert_eq!(client.balance_of(&token, &tag_a), 1_000_000);
    assert_eq!(client.balance_of(&token, &tag_b), 0);

    // Attempting to withdraw from Tag B must be rejected with InsufficientEscrowBalance (Error #3)
    let res = client.try_withdraw(&token, &tag_b, &payee_b, &500_000);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(vault::Error::InsufficientEscrowBalance as u32))),
        "Withdrawal from tag B must not touch collateral deposited under tag A"
    );

    // Verify tag A's collateral is intact
    assert_eq!(client.balance_of(&token, &tag_a), 1_000_000);
}

#[test]
fn threat_3_unregistered_integrator_cannot_create_tags() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin).address();

    let vault_id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &vault_id);
    client.initialize(&admin, &token);

    let rogue = Address::generate(&env);
    let rogue_tag = Tag {
        owner: rogue.clone(),
        kind: Symbol::new(&env, "series"),
        id: 1,
    };

    // Rogue is not registered via set_integrator -> UnregisteredIntegrator (Error #8)
    let res = client.try_deposit(&rogue, &token, &rogue_tag, &500_000);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(vault::Error::UnregisteredIntegrator as u32))),
        "Unregistered integrator cannot create tags"
    );
}

#[test]
fn threat_3_vault_delta_check_protects_against_fee_on_transfer_undercollateralization() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_id = env.register_contract(None, FeeOnTransferToken);
    let depositor = Address::generate(&env);

    // Initialize FeeOnTransferToken (10% fee on transfer)
    env.invoke_contract::<()>(
        &token_id,
        &Symbol::new(&env, "initialize"),
        vec![&env, depositor.to_val()],
    );
    env.invoke_contract::<()>(
        &token_id,
        &Symbol::new(&env, "mint"),
        vec![&env, depositor.to_val(), 1_000_000i128.into_val(&env)],
    );

    let vault_id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &vault_id);
    client.initialize(&admin, &token_id);
    client.set_integrator(&depositor, &true);

    let tag = Tag {
        owner: depositor.clone(),
        kind: Symbol::new(&env, "series"),
        id: 42,
    };

    // Depositor deposits 1,000,000 tokens.
    // 10% fee means only 900,000 arrives in vault.
    // Vault measures before/after delta and credits ONLY 900,000.
    let credited = client.deposit(&depositor, &token_id, &tag, &1_000_000);
    assert_eq!(
        credited, 900_000,
        "Vault must credit only actual received delta"
    );
    assert_eq!(client.balance_of(&token_id, &tag), 900_000);
    assert_eq!(client.get_total_escrowed(&token_id), 900_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 2.6 & Core Requirement 4: Multisig Signer Bypass & Quorum Enforcement
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn threat_4_signer_approving_outside_allowed_set_is_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let signers = vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let ms_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &ms_id);
    client.initialize(&signers, &2, &0);

    let stranger = Address::generate(&env);
    let action_id = BytesN::from_array(&env, &[77u8; 32]);

    // Stranger attempting approval -> NotASigner (Error #4)
    let res = client.try_approve(&stranger, &action_id);
    assert_eq!(
        res.err(),
        Some(Ok(multisig::Error::NotASigner)),
        "Non-signer approval must reject with NotASigner"
    );

    // Stranger attempting to propose signer change -> NotASigner (Error #4)
    let new_signer = Address::generate(&env);
    let res = client.try_propose_signer_change(
        &stranger,
        &vec![&env, SignerWeight { address: new_signer, weight: 1u32 }],
        &Vec::new(&env),
        &2,
        &3,
    );
    assert_eq!(
        res.err(),
        Some(Ok(multisig::Error::NotASigner)),
        "Non-signer proposing signer change must reject with NotASigner"
    );
}

#[test]
fn threat_4_multisig_action_execution_below_threshold_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let signers = vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let ms_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &ms_id);
    client.initialize(&signers, &2, &0); // 2-of-3 required

    let target = env.register_contract(None, MockAdminTarget);
    let id = client.propose(
        &signers.get(0).unwrap(),
        &target,
        &Symbol::new(&env, "execute_action"),
        &vec![&env, ms_id.to_val(), 10u32.into_val(&env)],
    );

    // Only 1 signer approves (below threshold of 2)
    client.approve(&signers.get(0).unwrap(), &id);
    assert!(!client.is_approved(&id));

    // Execution attempt below threshold -> NotYetApproved (Error #6)
    let res = client.try_execute(&id);
    assert_eq!(
        res.err(),
        Some(Ok(multisig::Error::NotYetApproved)),
        "Execution below threshold must reject with NotYetApproved"
    );
}

#[test]
fn threat_4_multisig_duplicate_approval_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let signers = vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
    ];
    let ms_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &ms_id);
    client.initialize(&signers, &2, &0);

    let action_id = BytesN::from_array(&env, &[12u8; 32]);
    client.approve(&signers.get(0).unwrap(), &action_id);

    // Replay approval from same signer -> AlreadyApproved (Error #5)
    let res = client.try_approve(&signers.get(0).unwrap(), &action_id);
    assert_eq!(
        res.err(),
        Some(Ok(multisig::Error::AlreadyApproved)),
        "Duplicate approval from same signer must reject with AlreadyApproved"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Section 1, 2.4 & Core Requirement 5: Timelock Early Execution & Delay Bypass
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn threat_5_timelock_action_executed_before_delay_fails_not_ready() {
    let env = Env::default();
    env.mock_all_auths();

    let proposer = Address::generate(&env);
    let executor = Address::generate(&env);
    let canceller = Address::generate(&env);

    let tl_id = env.register_contract(None, Timelock);
    let client = TimelockClient::new(&env, &tl_id);

    let delay = 86_400u64;
    client.initialize(
        &delay,
        &vec![&env, proposer.clone()],
        &vec![&env, executor.clone()],
        &vec![&env, canceller.clone()],
    );

    let target = env.register_contract(None, MockAdminTarget);
    let calls = vec![
        &env,
        Call {
            target,
            function: Symbol::new(&env, "execute_action"),
            args: vec![&env, tl_id.to_val(), 100u32.into_val(&env)],
        },
    ];
    let salt = BytesN::from_array(&env, &[55u8; 32]);

    let op_id = client.schedule(&proposer, &calls, &None, &salt, &delay);

    // Advance time by (delay - 1 second)
    env.ledger().set_timestamp(env.ledger().timestamp() + delay - 1);
    assert!(!client.is_operation_ready(&op_id));

    // Execute before delay has elapsed -> NotReady (Error #5)
    let res = client.try_execute(&executor, &calls, &None, &salt);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(timelock::Error::NotReady as u32))),
        "Executing timelock action before delay has elapsed must fail with NotReady"
    );

    // Advance 1 more second to exact ready_at boundary -> execution succeeds
    env.ledger().set_timestamp(env.ledger().timestamp() + 1);
    assert!(client.is_operation_ready(&op_id));
    client.execute(&executor, &calls, &None, &salt);
    assert!(client.is_operation_done(&op_id));
}

#[test]
fn threat_5_timelock_delay_below_minimum_fails_insufficient_delay() {
    let env = Env::default();
    env.mock_all_auths();

    let proposer = Address::generate(&env);
    let executor = Address::generate(&env);
    let canceller = Address::generate(&env);

    let tl_id = env.register_contract(None, Timelock);
    let client = TimelockClient::new(&env, &tl_id);

    let min_delay = 86_400u64;
    client.initialize(
        &min_delay,
        &vec![&env, proposer.clone()],
        &vec![&env, executor.clone()],
        &vec![&env, canceller.clone()],
    );

    let target = env.register_contract(None, MockAdminTarget);
    let calls = vec![
        &env,
        Call {
            target,
            function: Symbol::new(&env, "execute_action"),
            args: vec![&env, tl_id.to_val(), 1u32.into_val(&env)],
        },
    ];
    let salt = BytesN::from_array(&env, &[22u8; 32]);

    // Requesting delay < min_delay -> InsufficientDelay (Error #3)
    let res = client.try_schedule(&proposer, &calls, &None, &salt, &(min_delay - 1));
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(timelock::Error::InsufficientDelay as u32))),
        "Scheduling with delay below min_delay must reject with InsufficientDelay"
    );
}

#[test]
fn threat_5_timelock_dependent_cannot_execute_before_predecessor() {
    let env = Env::default();
    env.mock_all_auths();

    let proposer = Address::generate(&env);
    let executor = Address::generate(&env);
    let canceller = Address::generate(&env);

    let tl_id = env.register_contract(None, Timelock);
    let client = TimelockClient::new(&env, &tl_id);
    let delay = 3600u64;
    client.initialize(
        &delay,
        &vec![&env, proposer.clone()],
        &vec![&env, executor.clone()],
        &vec![&env, canceller.clone()],
    );

    let target = env.register_contract(None, MockAdminTarget);
    let calls_1 = vec![
        &env,
        Call {
            target: target.clone(),
            function: Symbol::new(&env, "execute_action"),
            args: vec![&env, tl_id.to_val(), 1u32.into_val(&env)],
        },
    ];
    let calls_2 = vec![
        &env,
        Call {
            target,
            function: Symbol::new(&env, "execute_action"),
            args: vec![&env, tl_id.to_val(), 2u32.into_val(&env)],
        },
    ];
    let salt_1 = BytesN::from_array(&env, &[10u8; 32]);
    let salt_2 = BytesN::from_array(&env, &[20u8; 32]);

    let pred_id = client.schedule(&proposer, &calls_1, &None, &salt_1, &delay);
    let ready_at_2 = env.ledger().timestamp() + delay;
    let _ = client.schedule(&proposer, &calls_2, &Some(pred_id.clone()), &salt_2, &delay);

    // Advance time past delay for both operations
    env.ledger().set_timestamp(ready_at_2);

    // Attempting to execute dependent operation before predecessor -> PredecessorNotDone (Error #6)
    let res = client.try_execute(&executor, &calls_2, &Some(pred_id), &salt_2);
    assert_eq!(
        res.err(),
        Some(Ok(contract_err(timelock::Error::PredecessorNotDone as u32))),
        "Dependent operation cannot execute before predecessor is done"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// THREAT_MODEL Gaps & Unimplemented Features (Tracked Follow-ups)
// ─────────────────────────────────────────────────────────────────────────────

/// THREAT_MODEL Section 2.1: Per-window cumulative outflow limits and automated
/// withdrawal circuit breakers on vault and options_market.
#[test]
#[ignore = "issue #19: rate-limited outflows and withdrawal circuit breakers not yet implemented"]
fn threat_unverified_section_2_1_rate_limited_outflow_circuit_breaker() {
    // Tracked follow-up: assert per-window cumulative outflow cap rejects
    // rapid liquidation drains once circuit breaker feature lands (issue #19).
}

/// THREAT_MODEL Section 2.2: Protocol Guardian auto-expiring pause mechanism.
#[test]
#[ignore = "issue #18: auto-expiring pauses and protocol guardian role not yet implemented"]
fn threat_unverified_section_2_2_protocol_guardian_auto_expiring_pauses() {
    // Tracked follow-up: assert pause automatically expires after MAX_PAUSE_DURATION
    // once Protocol Guardian role lands (issue #18).
}

/// THREAT_MODEL Section 2.3: Granular access control roles (GUARDIAN, ORACLE_ADMIN,
/// FEE_MANAGER, EMERGENCY_SETTLER in separate zenith-access crate).
#[test]
#[ignore = "issue #17: granular access control roles not yet implemented"]
fn threat_unverified_section_2_3_granular_access_control_roles() {
    // Tracked follow-up: assert separation of privileged roles once zenith-access
    // crate lands (issue #17).
}

/// THREAT_MODEL Section 2.6: Multisig timelock tiers (`is_executable` on multisig contract).
#[test]
#[ignore = "issue #150: multisig is_executable per-ActionClass delay enforcement not yet implemented"]
fn threat_unverified_section_2_6_multisig_timelock_tiers_is_executable() {
    // Tracked follow-up: assert per-class delays (Emergency=0, Standard=24h, Critical=72h)
    // are enforced directly on multisig contract entrypoints once issue #150 lands.
}
