//! Fuzz and property-based test harness for vault tag isolation and cross-contract escrow invariants.
//!
//! Implements Issue #199:
//! - Randomized sequence generator over ordered operations: `deposit`, `withdraw`, `transfer_tag`,
//!   allowlist changes, and cross-contract simulated `options_market` escrow/refund.
//! - Dual-ledger differential testing: maintains a reference ledger in the harness and verifies
//!   strict equality against on-chain contract state after every single operation.
//! - Invariant 1 (Tag Isolation): No operation on tag A can alter tag B's balance.
//! - Invariant 2 (Conservation): Contract total escrowed equals the sum of all individual tag balances.
//! - Invariant 3 (Atomic Rollback): Failed operations cause 0 state mutations (no partial updates).
//! - Deterministic seed replay with full operation trace on failure.
//! - Sanity-tested against deliberately injected state anomalies to ensure the harness fails closed.


extern crate std;

use std::format;
use std::string::String;
use std::vec;
use std::vec::Vec;

use crate::{Tag, Vault, VaultClient};
use soroban_sdk::{
    symbol_short,
    testutils::Address as _,
    token, Address, Env,
};

// ─── Deterministic Seeded PRNG (XorShift64) ──────────────────────────────────

struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0xdeadbeef_cafebabe } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn gen_range(&mut self, min: u64, max: u64) -> u64 {
        if min >= max {
            min
        } else {
            min + (self.next_u64() % (max - min))
        }
    }

    fn gen_i128_range(&mut self, min: i128, max: i128) -> i128 {
        if min >= max {
            min
        } else {
            min + (self.next_u64() as i128 % (max - min))
        }
    }

    fn gen_bool(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }
}

// ─── Reference Model ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct ReferenceEntry {
    token_idx: usize,
    tag_idx: usize,
    balance: i128,
}

struct ReferenceLedger {
    entries: Vec<ReferenceEntry>,
    token_allowed: Vec<bool>,
    integrator_allowed: Vec<bool>,
    tag_initialized: Vec<bool>,
}

impl ReferenceLedger {
    fn new(num_tokens: usize, num_tags: usize, num_integrators: usize) -> Self {
        let mut entries = Vec::new();
        for t in 0..num_tokens {
            for tg in 0..num_tags {
                entries.push(ReferenceEntry {
                    token_idx: t,
                    tag_idx: tg,
                    balance: 0,
                });
            }
        }
        Self {
            entries,
            token_allowed: vec![true; num_tokens],
            integrator_allowed: vec![true; num_integrators],
            tag_initialized: vec![false; num_tags],
        }
    }

    fn get_balance(&self, token_idx: usize, tag_idx: usize) -> i128 {
        self.entries
            .iter()
            .find(|e| e.token_idx == token_idx && e.tag_idx == tag_idx)
            .map(|e| e.balance)
            .unwrap_or(0)
    }

    fn set_balance(&mut self, token_idx: usize, tag_idx: usize, balance: i128) {
        if let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.token_idx == token_idx && e.tag_idx == tag_idx)
        {
            e.balance = balance;
        }
    }

    fn total_for_token(&self, token_idx: usize) -> i128 {
        self.entries
            .iter()
            .filter(|e| e.token_idx == token_idx)
            .map(|e| e.balance)
            .sum()
    }
}

// ─── Fuzz Operations ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
enum FuzzOp {
    Deposit {
        token_idx: usize,
        tag_idx: usize,
        user_idx: usize,
        amount: i128,
    },
    Withdraw {
        token_idx: usize,
        tag_idx: usize,
        to_idx: usize,
        amount: i128,
    },
    TransferTag {
        token_idx: usize,
        from_tag_idx: usize,
        to_tag_idx: usize,
        amount: i128,
    },
    SetTokenAllowed {
        token_idx: usize,
        allowed: bool,
    },
    SetIntegrator {
        integrator_idx: usize,
        allowed: bool,
    },
    CrossContractEscrowRefund {
        token_idx: usize,
        series_tag_idx: usize,
        position_tag_idx: usize,
        escrow_amount: i128,
        settle_amount: i128,
    },
}

// ─── Test Environment Context ───────────────────────────────────────────────

#[allow(dead_code)]
struct FuzzContext<'a> {
    env: Env,
    client: VaultClient<'a>,
    admin: Address,
    tokens: Vec<Address>,
    users: Vec<Address>,
    tags: Vec<Tag>,
    tag_integrator_idx: Vec<usize>,
    ref_ledger: ReferenceLedger,
    operation_log: Vec<String>,
}

impl<'a> FuzzContext<'a> {
    fn setup(env: &'a Env, num_tokens: usize, num_tags: usize, num_users: usize) -> Self {
        env.budget().reset_unlimited();
        env.mock_all_auths();

        let admin = Address::generate(env);
        let contract_id = env.register_contract(None, Vault);
        let client = VaultClient::new(env, &contract_id);

        let mut tokens = Vec::new();
        for _ in 0..num_tokens {
            let tok_admin = Address::generate(env);
            let tok = env.register_stellar_asset_contract_v2(tok_admin).address();
            tokens.push(tok);
        }

        // Initialize vault with default token
        client.initialize(&admin, &tokens[0]);

        // Allow all tokens
        for tok in &tokens {
            client.set_token_allowed(tok, &true);
        }

        // Setup users and fund them with tokens
        let mut users = Vec::new();
        for _ in 0..num_users {
            let u = Address::generate(env);
            for tok in &tokens {
                token::StellarAssetClient::new(env, tok).mint(&u, &100_000_000);
            }
            users.push(u);
        }

        // Setup integrators and tags
        let num_integrators = 3;
        let mut integrators = Vec::new();
        for _ in 0..num_integrators {
            let int_addr = Address::generate(env);
            client.set_integrator(&int_addr, &true);
            integrators.push(int_addr);
        }

        let mut tags = Vec::new();
        let mut tag_integrator_idx = Vec::new();
        for i in 0..num_tags {
            let int_idx = i % num_integrators;
            let kind = if i % 2 == 0 {
                symbol_short!("series")
            } else {
                symbol_short!("position")
            };
            tags.push(Tag {
                owner: integrators[int_idx].clone(),
                kind,
                id: (i as u64) + 100,
            });
            tag_integrator_idx.push(int_idx);
        }

        let ref_ledger = ReferenceLedger::new(num_tokens, num_tags, num_integrators);

        Self {
            env: env.clone(),
            client,
            admin,
            tokens,
            users,
            tags,
            tag_integrator_idx,
            ref_ledger,
            operation_log: Vec::new(),
        }
    }

    /// Read contract state and assert all invariants hold strictly against reference ledger.
    fn verify_invariants(&self, step: usize, seed: u64) {
        for (t_idx, tok) in self.tokens.iter().enumerate() {
            let mut sum_tag_balances = 0i128;

            for (tg_idx, tg) in self.tags.iter().enumerate() {
                let actual_bal = self.client.balance_of(tok, tg);
                let expected_bal = self.ref_ledger.get_balance(t_idx, tg_idx);

                assert_eq!(
                    actual_bal,
                    expected_bal,
                    "[Fuzz Invariant 1 Violation - Tag Isolation] Seed: {}, Step: {}\n\
                     Token idx: {}, Tag idx: {:?}\n\
                     Contract balance: {}, Reference balance: {}\n\
                     Recent log:\n{:#?}",
                    seed,
                    step,
                    t_idx,
                    tg,
                    actual_bal,
                    expected_bal,
                    &self.operation_log[self.operation_log.len().saturating_sub(10)..]
                );

                assert!(
                    actual_bal >= 0,
                    "[Fuzz Invariant Violation - Negative Balance] Seed: {}, Step: {}\n\
                     Tag: {:?}, Balance: {}",
                    seed,
                    step,
                    tg,
                    actual_bal
                );

                sum_tag_balances += actual_bal;
            }

            let contract_total = self.client.get_total_escrowed(tok);
            let expected_total = self.ref_ledger.total_for_token(t_idx);

            assert_eq!(
                contract_total,
                expected_total,
                "[Fuzz Invariant 2 Violation - Total Conservation] Seed: {}, Step: {}\n\
                 Token idx: {}\n\
                 Contract total: {}, Reference total: {}",
                seed,
                step,
                t_idx,
                contract_total,
                expected_total
            );

            assert_eq!(
                contract_total,
                sum_tag_balances,
                "[Fuzz Invariant 2 Violation - Sum of tags != total] Seed: {}, Step: {}\n\
                 Sum of tags: {}, Contract total: {}",
                seed,
                step,
                sum_tag_balances,
                contract_total
            );
        }
    }

    /// Execute a generated fuzz operation, verifying state transitions and invariant consistency.
    fn execute_op(&mut self, op: FuzzOp, step: usize, seed: u64) {
        self.operation_log
            .push(format!("Step {:03}: {:?}", step, op));

        // Snapshot contract state before mutation to verify cross-tag independence on failure
        let pre_balances: Vec<i128> = self
            .tags
            .iter()
            .map(|tg| {
                let t_idx = match op {
                    FuzzOp::Deposit { token_idx, .. }
                    | FuzzOp::Withdraw { token_idx, .. }
                    | FuzzOp::TransferTag { token_idx, .. }
                    | FuzzOp::CrossContractEscrowRefund { token_idx, .. } => token_idx,
                    _ => 0,
                };
                self.client.balance_of(&self.tokens[t_idx], tg)
            })
            .collect();

        match op {
            FuzzOp::Deposit {
                token_idx,
                tag_idx,
                user_idx,
                amount,
            } => {
                let tok = &self.tokens[token_idx];
                let tg = &self.tags[tag_idx];
                let u = &self.users[user_idx];

                let res = self.client.try_deposit(u, tok, tg, &amount);

                if let Ok(Ok(received)) = res {
                    let old_bal = self.ref_ledger.get_balance(token_idx, tag_idx);
                    self.ref_ledger
                        .set_balance(token_idx, tag_idx, old_bal + received);
                    self.ref_ledger.tag_initialized[tag_idx] = true;
                } else {
                    // Failing deposit must produce 0 state changes
                    for (other_tg_idx, tg_other) in self.tags.iter().enumerate() {
                        let current_bal = self.client.balance_of(tok, tg_other);
                        assert_eq!(
                            current_bal, pre_balances[other_tg_idx],
                            "State mutated on failed deposit at step {}",
                            step
                        );
                    }
                }
            }

            FuzzOp::Withdraw {
                token_idx,
                tag_idx,
                to_idx,
                amount,
            } => {
                let tok = &self.tokens[token_idx];
                let tg = &self.tags[tag_idx];
                let to = &self.users[to_idx];

                let res = self.client.try_withdraw(tok, tg, to, &amount);

                if let Ok(Ok(())) = res {
                    let current_bal = self.ref_ledger.get_balance(token_idx, tag_idx);
                    self.ref_ledger
                        .set_balance(token_idx, tag_idx, current_bal - amount);
                } else {
                    // Failing withdrawal must produce 0 state changes
                    for (other_tg_idx, tg_other) in self.tags.iter().enumerate() {
                        let post_bal = self.client.balance_of(tok, tg_other);
                        assert_eq!(
                            post_bal, pre_balances[other_tg_idx],
                            "State mutated on failed withdrawal at step {}",
                            step
                        );
                    }
                }
            }

            FuzzOp::TransferTag {
                token_idx,
                from_tag_idx,
                to_tag_idx,
                amount,
            } => {
                let tok = &self.tokens[token_idx];
                let from_tg = &self.tags[from_tag_idx];
                let to_tg = &self.tags[to_tag_idx];

                let res = self.client.try_transfer_tag(tok, from_tg, to_tg, &amount);

                if let Ok(Ok(())) = res {
                    let from_bal = self.ref_ledger.get_balance(token_idx, from_tag_idx);
                    self.ref_ledger
                        .set_balance(token_idx, from_tag_idx, from_bal - amount);
                    let to_bal = self.ref_ledger.get_balance(token_idx, to_tag_idx);
                    self.ref_ledger
                        .set_balance(token_idx, to_tag_idx, to_bal + amount);
                } else {
                    // Failing transfer must produce 0 state changes
                    for (other_tg_idx, tg_other) in self.tags.iter().enumerate() {
                        let post_bal = self.client.balance_of(tok, tg_other);
                        assert_eq!(
                            post_bal, pre_balances[other_tg_idx],
                            "State mutated on failed transfer at step {}",
                            step
                        );
                    }
                }
            }

            FuzzOp::SetTokenAllowed { token_idx, allowed } => {
                let tok = &self.tokens[token_idx];
                self.client.set_token_allowed(tok, &allowed);
                self.ref_ledger.token_allowed[token_idx] = allowed;
            }

            FuzzOp::SetIntegrator {
                integrator_idx,
                allowed,
            } => {
                let int_addr = &self.tags[integrator_idx].owner;
                self.client
                    .set_integrator(int_addr, &allowed);
                self.ref_ledger.integrator_allowed[integrator_idx] = allowed;
            }

            FuzzOp::CrossContractEscrowRefund {
                token_idx,
                series_tag_idx,
                position_tag_idx,
                escrow_amount,
                settle_amount,
            } => {
                let tok = &self.tokens[token_idx];
                let series_tg = &self.tags[series_tag_idx];
                let pos_tg = &self.tags[position_tag_idx];

                // 1. Integrator locks escrow into series tag
                let u = &self.users[0];
                let res = self.client.try_deposit(u, tok, series_tg, &escrow_amount);
                if let Ok(Ok(received)) = res {
                    let old = self.ref_ledger.get_balance(token_idx, series_tag_idx);
                    self.ref_ledger
                        .set_balance(token_idx, series_tag_idx, old + received);
                    self.ref_ledger.tag_initialized[series_tag_idx] = true;
                }

                // 2. Options settlement transfers a portion to position tag
                let res2 = self
                    .client
                    .try_transfer_tag(tok, series_tg, pos_tg, &settle_amount);
                if let Ok(Ok(())) = res2 {
                    let current_series = self.ref_ledger.get_balance(token_idx, series_tag_idx);
                    self.ref_ledger.set_balance(
                        token_idx,
                        series_tag_idx,
                        current_series - settle_amount,
                    );
                    let old_pos = self.ref_ledger.get_balance(token_idx, position_tag_idx);
                    self.ref_ledger
                        .set_balance(token_idx, position_tag_idx, old_pos + settle_amount);
                }
            }
        }

        // Assert all invariants after this step
        self.verify_invariants(step, seed);
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn test_fuzz_tag_isolation_and_escrow_invariants_seed_1337() {
    run_fuzz_campaign(1337, 60);
}

#[test]
fn test_fuzz_tag_isolation_and_escrow_invariants_seed_42() {
    run_fuzz_campaign(42, 60);
}

#[test]
fn test_fuzz_tag_isolation_and_escrow_invariants_seed_9999() {
    run_fuzz_campaign(9999, 60);
}

#[test]
fn test_fuzz_tag_isolation_and_escrow_invariants_seed_cafe() {
    run_fuzz_campaign(0xcafebabe, 60);
}

#[test]
fn test_fuzz_tag_isolation_and_escrow_invariants_seed_deadbeef() {
    run_fuzz_campaign(0xdeadbeef, 60);
}

/// High-iteration nightly fuzz test, marked #[ignore] so standard CI stays fast.
#[test]
#[ignore]
fn test_fuzz_tag_isolation_high_iteration_nightly() {
    run_fuzz_campaign(0xfeedface_12345678, 1000);
}

/// Negative test required by Issue #199 acceptance criteria:
/// Verifies that if an artificial balance mutation/violation occurs, the harness
/// detects it immediately and panics with an invariant violation.
#[test]
#[should_panic(expected = "Fuzz Invariant 1 Violation - Tag Isolation")]
fn test_fuzz_harness_catches_tag_isolation_violation() {
    let env = Env::default();
    let mut ctx = FuzzContext::setup(&env, 2, 4, 3);

    // Initial valid deposit
    ctx.execute_op(
        FuzzOp::Deposit {
            token_idx: 0,
            tag_idx: 0,
            user_idx: 0,
            amount: 500,
        },
        0,
        1,
    );

    // Deliberately tamper with reference ledger to simulate tag isolation bypass
    ctx.ref_ledger.set_balance(0, 1, 99999);

    // This check must catch the discrepancy and panic
    ctx.verify_invariants(1, 1);
}

// ─── Campaign Runner ────────────────────────────────────────────────────────

fn run_fuzz_campaign(seed: u64, num_steps: usize) {
    let env = Env::default();
    let num_tokens = 2;
    let num_tags = 4;
    let num_users = 3;

    let mut ctx = FuzzContext::setup(&env, num_tokens, num_tags, num_users);
    let mut rng = Rng::new(seed);

    for step in 0..num_steps {
        let op_type = rng.gen_range(0, 6);
        let token_idx = rng.gen_range(0, num_tokens as u64) as usize;
        let tag_idx = rng.gen_range(0, num_tags as u64) as usize;
        let other_tag_idx = rng.gen_range(0, num_tags as u64) as usize;
        let user_idx = rng.gen_range(0, num_users as u64) as usize;

        let op = match op_type {
            0 => {
                // Deposit (including 0, negative, and large amounts)
                let amount = match rng.gen_range(0, 4) {
                    0 => 0,
                    1 => -10,
                    2 => rng.gen_i128_range(1, 5_000),
                    _ => rng.gen_i128_range(5_000, 20_000),
                };
                FuzzOp::Deposit {
                    token_idx,
                    tag_idx,
                    user_idx,
                    amount,
                }
            }
            1 => {
                // Withdraw (including overdrawing, 0, negative, and partial valid withdrawals)
                let current_bal = ctx.ref_ledger.get_balance(token_idx, tag_idx);
                let amount = match rng.gen_range(0, 4) {
                    0 => 0,
                    1 => current_bal + 50, // overdraw
                    2 => {
                        if current_bal > 0 {
                            rng.gen_i128_range(1, current_bal + 1)
                        } else {
                            100
                        }
                    }
                    _ => current_bal, // full withdrawal
                };
                FuzzOp::Withdraw {
                    token_idx,
                    tag_idx,
                    to_idx: user_idx,
                    amount,
                }
            }
            2 => {
                // TransferTag (reallocating between tags)
                let from_bal = ctx.ref_ledger.get_balance(token_idx, tag_idx);
                let amount = if from_bal > 0 {
                    rng.gen_i128_range(1, from_bal + 1)
                } else {
                    50
                };
                FuzzOp::TransferTag {
                    token_idx,
                    from_tag_idx: tag_idx,
                    to_tag_idx: other_tag_idx,
                    amount,
                }
            }
            3 => {
                // SetTokenAllowed toggle
                let allowed = rng.gen_bool();
                FuzzOp::SetTokenAllowed { token_idx, allowed }
            }
            4 => {
                // SetIntegrator toggle
                let integrator_idx = rng.gen_range(0, 3) as usize;
                let allowed = rng.gen_bool();
                FuzzOp::SetIntegrator {
                    integrator_idx,
                    allowed,
                }
            }
            _ => {
                // Simulating options_market settlement workflow
                let escrow_amount = rng.gen_i128_range(100, 2_000);
                let settle_amount = rng.gen_i128_range(50, 1_000);
                FuzzOp::CrossContractEscrowRefund {
                    token_idx,
                    series_tag_idx: tag_idx,
                    position_tag_idx: other_tag_idx,
                    escrow_amount,
                    settle_amount,
                }
            }
        };

        ctx.execute_op(op, step, seed);
    }
}
