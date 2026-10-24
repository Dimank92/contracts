# Panic-path policy

A panic aborts the whole Soroban transaction, so contract code (everything
outside `#[cfg(test)]` / `test.rs`) must not use `unwrap()`, `expect()`,
`panic!`, `todo!`, `unreachable!` or `unimplemented!`.

Enforced by the workspace clippy lints in the root `Cargo.toml`
(`[workspace.lints.clippy]`, deny level; `clippy.toml` exempts test code).
`lp_pool`, `lp_token` and `timelock` are outside the workspace and carry the
same table in their own manifests. Run locally with `cargo xtask clippy`
(CI's `clippy` job runs the same command).

## Replacing a panic

Guaranteed-present values (anything `initialize` always writes) and impossible
branches raise `Error::InternalInvariant` (code **90** in every Zenith
contract; `zenith_common::INTERNAL_INVARIANT`) via
`.unwrap_or_else(|| panic_with_error!(&env, Error::InternalInvariant))`.
Domain failures keep their own variants.

## Allow-list

A provably infallible site (e.g. building a `Vec` from a static slice) may use
`#[allow(clippy::unwrap_used)]` on the smallest item, with a comment
justifying why it cannot fail. Reviewers must approve each. Currently: none.
