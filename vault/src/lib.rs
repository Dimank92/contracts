// Public API docs are enforced by `cargo xtask doc` (#268): the gate builds
// rustdoc, where this lint is on, and only there — soroban-sdk macro output is
// undocumented, which would otherwise fail `clippy -D warnings`.
#![cfg_attr(doc, warn(missing_docs))]
//! Zenith vault — implementation stub.
//!
//! The `token_validation` module is fully implemented (used in tests);
//! test modules are declared here.

/// Collateral token transfer validation.
pub mod token_validation;

#[cfg(test)]
mod test_adversaries;
