//! Intentionally empty: see tests/handover.rs.
//!
//! Issue #222: this crate is temporarily excluded from the workspace in the
//! root `Cargo.toml` (dated 2024-06-01) because the `options_market`, `vault`,
//! and `timelock` crates do not currently compile. The integration tests that
//! depend on those entrypoints are quarantined with `#[ignore = "..."]`
//! annotations naming the missing entrypoint and the reconstruction issue that
//! will unblock them. Once those crates are reconstructed, remove the workspace
//! exclusion and drop the corresponding `#[ignore]` attributes.
