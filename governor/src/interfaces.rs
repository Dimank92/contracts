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
///
/// The timelock derives an operation's id from the batch it carries, the
/// predecessor it waits on and a salt, so the governor cannot name an
/// operation directly: it queues a proposal under the proposal's own id as
/// the salt and derives the operation id from the same inputs.
#[allow(dead_code)]
#[contractclient(name = "TimelockClient")]
pub trait Timelock {
    fn schedule(
        env: Env,
        proposer: Address,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        salt: BytesN<32>,
        delay: u64,
    ) -> BytesN<32>;
    fn hash_operation(
        env: Env,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
    ) -> BytesN<32>;
    fn is_operation_done(env: Env, id: BytesN<32>) -> bool;
    fn cancel(env: Env, canceller: Address, id: BytesN<32>);
    fn get_min_delay(env: Env) -> u64;
    fn get_grace_period(env: Env) -> u64;
    fn get_timestamp(env: Env, id: BytesN<32>) -> u64;
}
