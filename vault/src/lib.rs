//! # Checks-effects-interactions checklist
//!
//! All escrow and tag mutations must share a single `zenith_common::guard`
//! instance key, acquired before auth, multisig, or token balance calls.
//! Validate permissions/amounts and arithmetic before writing; debit liabilities
//! before outbound transfers. For inbound balance-delta accounting, hold the
//! guard over both balance reads, the transfer, and final credit. Propagate
//! failures as contract errors so writes and transfers roll back. Read-only
//! paths must not expose half-updated balances during an interaction.
//!
//! Integration is pending: this checkout has no vault entrypoints yet.

// vault lib.rs — implementation stub.
// The token_validation module is fully implemented (used in tests);
// test modules are declared here.

pub mod token_validation;

#[cfg(test)]
mod test_adversaries;
