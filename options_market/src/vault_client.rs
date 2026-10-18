//! Cross-contract client for vault, same pattern and same reason as
//! multisig_client.rs and price_oracle_client.rs: contractimport! against
//! vault's compiled wasm, not a normal source dependency, to avoid linking
//! vault's own #[contractimpl] functions into options_market's wasm and
//! colliding on shared names (pause/transfer_admin again).
//!
//! Requires vault's wasm in the shared workspace target/ before this crate
//! can compile at all; `cargo xtask build` handles that (see
//! `[package.metadata.zenith] wasm-deps` in ../Cargo.toml).
soroban_sdk::contractimport!(
    file = "../target/wasm32-unknown-unknown/release/zenith_vault.wasm"
);

/// The vault tag that scopes all of one series' funds: owned by this
/// contract, kind `"series"`, id = series_id. Used both for cancellation
/// refunds and for settlement claims (a series is either cancelled or
/// settled, never both).
pub fn series_tag(env: &soroban_sdk::Env, series_id: u64) -> Tag {
    Tag {
        owner: env.current_contract_address(),
        kind: soroban_sdk::symbol_short!("series"),
        id: series_id,
    }
}
