//! Cross-contract client for multisig, same pattern and same reason as
//! price_oracle_client.rs: contractimport! against multisig's compiled
//! wasm, not a normal source dependency, to avoid linking multisig's own
//! #[contractimpl] functions into options_market's wasm and colliding on
//! shared names.
//!
//! Requires multisig's wasm to already be built (`cd ../multisig &&
//! cargo build --target wasm32-unknown-unknown --release`) before this
//! crate can compile at all — see the repo README for the build order
//! this implies.
soroban_sdk::contractimport!(
    file = "../target/wasm32-unknown-unknown/release/zenith_multisig.wasm"
);
