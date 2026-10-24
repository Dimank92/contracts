# Reconstruction Roadmap & Stub Inventory

This is a living document. It records the **actual** state of the tree at write time
(not the README's aspirational description), the ordered dependency of reconstruction
work, and which issue owns each un-`#[ignore]`ing / non-compiling commitment.

Update this file as issues land. Prefer cross-links to issue numbers over prose.

## Per-crate status

Status legend: **implemented** (works), **stub** (compiles but no real logic),
**empty** (0-byte or placeholder file), **non-compiling** (does not build).

| Crate | Documented behavior (README / THREAT_MODEL) | File-level reality | Status |
|-------|---------------------------------------------|--------------------|--------|
| `price_oracle` | Price feeds, oracle aggregation, staleness checks | `src/lib.rs` is 0 bytes | empty |
| `vault` | Deposit/withdraw escrow, events, typed errors, tests | `src/lib.rs` is an 8-line stub; `test.rs`, `types.rs`, `events.rs`, `error.rs` are empty | stub |
| `timelock` | Versioned storage scheme, admin-gated `migrate` entrypoint | `src/lib.rs` + `src/types.rs` present (see #208 batch); `migrations` shim only otherwise | stub |
| `options_market` | Options issuance, escrow wiring, math, storage | `src/lib.rs` does not compile; `math.rs` and `storage.rs` are empty | non-compiling |
| `staking` | Staking rewards, accounting fixes | Fixes tracked separately (see below) | partial |
| `streams` | Streaming payments, accounting fixes | Fixes tracked separately (see below) | partial |

## README ↔ tree mismatches (explicit)

- `price_oracle/src/lib.rs` — README describes a working oracle; file is **0 bytes**.
- `vault/src/lib.rs` — README describes full escrow; file is an **8-line stub**, and
  `test.rs` / `types.rs` / `events.rs` / `error.rs` are **empty**.
- `timelock/` — README describes a timelock contract; directory is **empty except a
  migrations shim** (plus the `lib.rs`/`types.rs` added in this batch).
- `options_market/src/lib.rs` — README describes a working options market; the file
  **does not compile**, and `math.rs` / `storage.rs` are **empty**.

## Ordered dependency of reconstruction work

1. **`price_oracle`** — implement `src/lib.rs` (types → storage → entrypoints).
2. **`vault`** — `types.rs` → `events.rs` → `error.rs` → `lib.rs` entrypoints → `test.rs`.
3. **`timelock`** — `types.rs` → versioned storage → admin-gated `migrate` entrypoint.
4. **`options_market`** — `math.rs` → `storage.rs` → `lib.rs` (compiles) → escrow wiring.
   Escrow wiring **depends on `vault`** being implemented first.
5. **`staking` / `streams`** — accounting fixes (independent of the above).

Rule of thumb: **storage/types before entrypoints**; **`vault` before
`options_market` escrow wiring**.

## `#[ignore]`'d / non-compiling test suites and owning issues

| Suite | State | Owning issue |
|-------|-------|--------------|
| `vault` tests (`vault/src/test.rs`) | `#[ignore]`'d / empty | #115 |
| `options_market` tests | non-compiling (crate does not build) | tracked with `options_market` reconstruction |
| `price_oracle` tests | none (empty crate) | tracked with `price_oracle` reconstruction |
| `timelock` tests | none (stub) | tracked with `timelock` reconstruction |

## Out of scope

Fixing the crates themselves. This document is inventory and planning only.
