// Run guard regressions independently of the pre-existing migration test
// harness, which currently does not compile against the SDK's try_* types.
#[path = "../src/guard.rs"]
mod guard;
