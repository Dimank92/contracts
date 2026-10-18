//! Cross-contract client for the params registry, same pattern and same
//! reason as price_oracle_client.rs: contractimport! against params'
//! compiled wasm, not a normal source dependency.
//!
//! Requires params' wasm in the shared workspace target/ before this crate
//! can compile at all; `cargo xtask build` handles that (see
//! `[package.metadata.zenith] wasm-deps` in ../Cargo.toml).
soroban_sdk::contractimport!(file = "../target/wasm32-unknown-unknown/release/zenith_params.wasm");
