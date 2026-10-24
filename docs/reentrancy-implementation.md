# Reentrancy guard: partial implementation and validation

Base revision: `61b3c5410e8726be371ae8dd210408db1a9c63d8`.

## Status: NOT a complete fix

The reusable primitive is implemented and its focused regression tests pass.
This checkout cannot support the requested end-to-end integration without
reconstructing missing contract implementations. No claim of complete issue
resolution, >90% end-to-end confidence, passing CI, PR submission, or review
approval is made.

## Implemented

- `zenith_common::guard::with_guard(env, key, violation, closure)` uses instance
  storage and one read to check a namespaced flag. A nested acquisition returns
  the supplied distinct error without executing the closure or removing the
  outer lock. All ordinary return paths remove the flag, including `?`/`Err`.
- Failed host invocations roll back acquisition; cleanup does not rely on Drop
  or unwinding. A regression contract tests rollback after a panic.
- `checks_effects_interactions(checks, effects, interactions)` orders and
  short-circuits stages. It must run inside the lock.
- Tests cover release, nested acquisitions, preservation of the outer lock,
  independent classes, panic rollback, ordering and failed-stage short-circuit.
- Crate-doc checklists in vault and options_market explain intended integration.
- No vault tag layout, authorization shape, contract ABI, or adversary mocks
  were changed.

## Integration contract

Choose a single shared key for ALL entrypoints affecting the same liabilities.
Different per-method keys would permit deposit -> withdraw interleaving.
Use a dedicated, unused contract error discriminant for the `violation` argument.
Acquire before `require_auth`, multisig authorization, token `balance`, oracle
queries, or vault calls. Keep `Auth::Multisig` arguments unchanged inside the
protected closure. Validate before local effects and debit claims/mark terminal
states before payouts. Inbound balance-delta validation is an unavoidable
interaction-before-final-credit exception: hold the lock across the entire
sequence. Propagate any failure through a contract `Result` or a panic; a plain
Rust helper returning Err does not undo state or token transfers by itself.

The instance tuple namespace `(Symbol("zenith_guard"), key)` is reserved.
No persistent entry or separate TTL maintenance is used. Calls sharing state
must use the same key; batch implementations must acquire once and use private
inner functions rather than invoke guarded public wrappers recursively.
Read-only callbacks must not expose inconsistent accounting mid-interaction.

## Existing blockers and remaining work

- `vault/src/lib.rs` originally has eight lines: a token_validation export and
  adversary test declaration, but no Vault, VaultClient, deposit, withdraw,
  tag-mutating entrypoints, or contract implementation. `error.rs`, `types.rs`,
  and `test.rs` are empty. There is no tag layout to safely integrate against.
- `options_market/src/lib.rs` originally has 63 lines with a missing semicolon,
  misplaced inner documentation, duplicated modules/imports, and no contract
  entrypoints. Restoring implementations is not a mechanical guard fix.
- The existing common migration tests use typed errors where generated try_*
  clients return SDK errors; all 12 mismatches remain outside this change.
- The adversarial crate has duplicate definitions and incompatible invocation
  result types (workspace test attempt reports 420 errors).

After restoring the intended implementation revision, guard every shared-state
mutator (including admin/multisig twins), assign distinct error codes, add a
syntax-aware coverage gate that inventories actual entrypoints, and execute
real deposit/withdraw/settlement re-entry tests. Neither those contract-level
regressions nor the new-entrypoint coverage gate is implemented here: reporting
zero entrypoints as full coverage would be misleading. The requested malicious
cross-contract mocks/harness were explicitly out of scope and were not added.

## Validation

Rust was not installed initially; installed rustc 1.99.1/cargo via rustup.
Fresh dependency resolution selected ed25519-dalek 3 for soroban-env-host's
broad version requirement, causing an incompatible RNG trait error. Generated
Cargo.lock and ran `cargo update -p ed25519-dalek@3.0.0 --precise 2.2.0`.
The lockfile records a reproducible SDK-compatible resolution without modifying
manifests or vendoring third-party source.

| Command | Result |
| --- | --- |
| `cargo build -p zenith-common` | PASS |
| `cargo test -p zenith-common --test guard` | PASS: 4 tests, 0 failed/ignored |
| `cargo test -p zenith-common` | BLOCKED: 12 pre-existing migration test type errors |
| `cargo build --workspace` | FAIL: malformed options_market source (34 errors reported) |
| `cargo test --workspace` | FAIL: existing adversary compilation errors |

`common/tests/guard.rs` compiles the guard module independently so its tests can
run despite the unrelated migration unit test compilation failures. These are
primitive tests, NOT malicious-token callback tests. Soroban host-level
re-entry restrictions also mean a rejected cross-contract callback cannot
necessarily surface as a custom contract guard error; that must be established
against the intended runtime/harness, not assumed from these tests.

Raw test/build output is in `docs/validation/reentrancy/`. Full workspace tests
cannot execute until compilation blockers are resolved. The work is local in
`/home/user/contracts`; no remote branch, GitHub PR, or remote codespace was
modified.
