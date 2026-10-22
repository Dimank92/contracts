//! Cross-contract interfaces. Declared as traits rather than
//! `contractimport!`-ed so this crate needs no other wasm to build.

use soroban_sdk::{contractclient, Address, BytesN, Env, Symbol, Val, Vec};

/// Checkpointed voting token (ZEN). Timepoints are ledger sequence
/// numbers and must be strictly in the past.
#[allow(dead_code)]
#[contractclient(name = "VotesClient")]
pub trait Votes {
    fn get_past_votes(env: Env, account: Address, ledger: u32) -> i128;
    fn get_past_total_supply(env: Env, ledger: u32) -> i128;
}

/// Subset of the Zenith timelock used by the governor.
#[allow(dead_code)]
#[contractclient(name = "TimelockClient")]
pub trait Timelock {
    fn schedule(
        env: Env,
        op_id: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
    ) -> u64;
    fn execute(
        env: Env,
        op_id: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
    );
    fn is_done(env: Env, op_id: BytesN<32>) -> bool;
    fn is_pending(env: Env, op_id: BytesN<32>) -> bool;
    fn cancel(env: Env, caller: Address, op_id: BytesN<32>);
    fn get_min_delay(env: Env) -> u64;
    fn get_grace_period(env: Env) -> u64;
}