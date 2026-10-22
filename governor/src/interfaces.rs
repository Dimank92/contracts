//! Cross-contract interfaces. Declared as traits rather than
//! `contractimport!`-ed so this crate needs no other wasm to build.

use soroban_sdk::{contractclient, Address, BytesN, Env, Symbol, Val, Vec};

/// Checkpointed voting token (ZEN). Timepoints are ledger sequence
/// numbers and must be strictly in the past.
#[allow(dead_code)]
/// Voting-power snapshot interface of the governance token.
#[contractclient(name = "VotesClient")]
pub trait Votes {
    /// Voting power of `account` at `ledger`.
    fn get_past_votes(env: Env, account: Address, ledger: u32) -> i128;
    /// Total voting supply at `ledger`.
    fn get_past_total_supply(env: Env, ledger: u32) -> i128;
}

/// Subset of the Zenith timelock used by the governor.
#[allow(dead_code)]
/// Timelock operations the governor depends on.
#[contractclient(name = "TimelockClient")]
pub trait Timelock {
    /// Schedules an operation and returns its id.
    fn schedule(
        env: Env,
        op_id: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
    ) -> u64;
    /// Whether operation `op_id` has been executed.
    fn is_done(env: Env, op_id: BytesN<32>) -> bool;
    /// Cancels operation `op_id` on behalf of `caller`.
    fn cancel(env: Env, caller: Address, op_id: BytesN<32>);
    /// Minimum delay before a scheduled operation can run.
    fn get_min_delay(env: Env) -> u64;
    /// Window after the delay during which an operation can still run.
    fn get_grace_period(env: Env) -> u64;
}
