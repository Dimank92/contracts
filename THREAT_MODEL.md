# Zenith Protocol Formal Threat Model & Adversarial Attack Suite

This document defines the formal threat model, security invariants, trust
boundaries, and adversarial test scenarios across the Zenith options protocol
contracts (`options_market`, `price_oracle`, `vault`, `multisig`).

> **Accuracy notice (issue #186).** Several mitigations described in this
> document depend on contracts or contract features that are **not yet
> implemented** (see the Crate status table in README.md). Each unverified
> mitigation is flagged with a `⚠ UNVERIFIED` marker and a pointer to the
> tracking issue. The flags will be removed as the corresponding work lands.

---

## 1. Trust Boundaries & Privileged Actors

- **Untrusted Actors**: Any external caller executing permissionless
  entrypoints (`deposit`, `buy_option`, `exercise`, `settle`).
- **Oracle Feeders**: Whitelisted price reporters providing asset spot prices
  under the `price_oracle` median aggregate.
- **Protocol Guardian / Admin**: A single `admin: Address` on each contract;
  intended to be the timelock once governance is wired up.
- **Multisig**: Quorum-governed M-of-N approval, checked via `is_approved`.
  Timelock-tier delays (`is_executable`) are not yet implemented — see
  issue #150.

---

## 2. Core Security Invariants & Attack Mitigations

### 2.1 Rate-Limited Outflows & Circuit Breakers

> ⚠ **UNVERIFIED — depends on unbuilt contracts (issue #115, #19).**
> Per-window cumulative outflow limits and automated withdrawal circuit
> breakers described below are **not implemented** in either `vault` or
> `options_market` today. `vault/src/lib.rs` is an 8-line stub with no
> entrypoints; `options_market` has no outflow rate-limiting logic.
> This mitigation does not exist in the deployed codebase.

- **Attack Vector**: Rapid liquidation or reentrancy exploit attempting to
  drain vault capital in a single ledger window.
- **Intended Invariant**: Per-window cumulative outflow limits enforce an
  automated withdrawal circuit breaker on both `vault` and `options_market`.
- **Current State**: No circuit breaker exists. The only withdrawal guard is
  the per-position collateral accounting in `options_market`.

### 2.2 Protocol Guardian & Auto-Expiring Pauses

> ⚠ **UNVERIFIED — depends on unbuilt features (issue #18).**
> Auto-expiring pauses and a `Protocol Guardian` role with automatic
> time-based expiration are **not implemented**. `options_market` has
> `pause()`/`unpause()` (admin-only, no expiry), and `vault`'s pause has
> a `max_pause_duration` escape-hatch for beneficiaries — but neither
> contract enforces an automatic unpause. The `_via_multisig` alternatives
> for pause/unpause exist in `options_market` but not with time-bounded
> enforcement. The timelock (`timelock/src/lib.rs` is empty) cannot enforce
> bounded pause durations yet.

- **Attack Vector**: Indefinite admin lockout or griefing via permanent
  contract pause.
- **Intended Invariant**: Pauses automatically expire after
  `MAX_PAUSE_DURATION` unless ratified by multisig.
- **Current State**: Pauses are indefinite admin actions. The vault's
  `emergency_withdraw` provides an escape hatch for registered beneficiaries
  after `max_pause_duration`, but that is a user-opt-in path, not an
  automatic unpause.

### 2.3 Granular Access Control

> ⚠ **UNVERIFIED — depends on unbuilt features (issue #17, #150).**
> A separate `zenith-access` crate with `GUARDIAN`, `ORACLE_ADMIN`,
> `FEE_MANAGER`, and `EMERGENCY_SETTLER` roles is **not implemented**.
> The multisig timelock tiers (`is_executable`, `ActionClass`) that would
> enforce per-class delays are also not yet built — see issue #150.
> All contracts currently have a single `admin: Address`.

- **Attack Vector**: Single admin private key compromise exposing all
  contracts.
- **Intended Invariant**: Strict separation between `GUARDIAN`,
  `ORACLE_ADMIN`, `FEE_MANAGER`, and `EMERGENCY_SETTLER` roles.
- **Current State**: Each contract has one `admin`. The `_via_multisig`
  entrypoints exist as an additive alternative (M-of-N approval), but no
  role separation is enforced, and `is_executable` timelocks are absent.

### 2.4 Flash-Loan / Flash-Vote Resistance

- **Flash-Loan Resistance (price manipulation)**: Settlement price
  calculation in `options_market` is gated by `set_settlement_price` or
  `set_settlement_price_from_oracle`. The `from_oracle` path cross-calls
  `price_oracle`, which aggregates a median across multiple feeder reports —
  however, `price_oracle/src/lib.rs` is currently 0 bytes (stub), so
  this path is not callable today. The trusted-oracle path
  (`set_settlement_price`) sets price via a single admin-authorized address,
  with no same-ledger manipulation protection beyond the admin trust model.

  > ⚠ **PARTIAL — `price_oracle` is a stub (issue #115).** The feeder
  > consensus path that would resist price manipulation is not yet deployed.

- **Flash-Vote Resistance (governance)**: `governor` reads voting power at
  the proposal's snapshot ledger, which is necessarily in the past by the
  time voting opens. Tokens acquired after proposal creation carry no weight.
  This resistance is **implemented** in `governor/src/lib.rs`. However,
  `governor::queue` depends on `timelock::schedule`, and `timelock` is a
  stub — so proposals cannot be queued/executed on-chain yet (issue #115).

### 2.5 Solvency Invariant

- **Invariant**: Total collateral locked in `options_market` must always
  equal or exceed the protocol's aggregate option liabilities.
- **Current State**: `options_market` tracks `PremiumPool`, per-position
  `collateral_locked`, and `SeriesEscrow`. The invariant is maintained
  within `options_market`'s own accounting. However, because `vault` is a
  stub, the per-tag escrow isolation (which would prevent cross-series
  collateral leakage) is not active. The "Known gaps" section of the README
  describes the cancellation accounting gap that results.

### 2.6 Multisig Timelock Tiers

> ⚠ **UNVERIFIED — not yet implemented (issue #150).** The `is_executable`
> function and per-`ActionClass` delay enforcement described in the `multisig`
> reference are **not committed**. `zenith-common`'s
> `require_admin_or_multisig` currently calls `is_approved` (not
> `is_executable`), so no timelocked multisig path is enforced today.
> All `_via_multisig` entrypoints effectively have zero mandatory delay.

- **Attack Vector**: A compromised M-of-N quorum executing a Critical-class
  action (upgrade, admin transfer) instantly with no observation window.
- **Intended Mitigation**: Per-class delays (`Emergency` = 0, `Standard` =
  24h, `Critical` = 72h) enforced by `is_executable`.
- **Current State**: No delay is enforced beyond what the timelock would add
  if wired up (and the timelock is also a stub).

---

## 3. Adversarial Test Suite

`test_adversaries/` implements 20 adversarial contracts across four modules
(`tokens`, `oracle`, `vault`, `multisig`). Each spoofs a real contract
interface with worst-case behaviors (lying balances, revert-on-transfer,
always-approve multisig, etc.). Tests using these adversaries that expose
**open vulnerabilities** are `#[ignore]`d with their tracking issue number
and un-ignored when the fix lands.

See [`test_adversaries/CATALOG.md`](test_adversaries/CATALOG.md) for the
full catalog. Current ignore groups:

- `issue #115` — tests requiring a live `price_oracle` or `vault` instance
  (those contracts are stubs)
- `issue #115` — tests requiring `options_market::set_settlement_price_from_oracle`
  (depends on price_oracle)

The CI `workspace-compile` job (`cargo xtask check-ignore`) enforces that
every `#[ignore]` carries a tracking reference, preventing undocumented
test skips.

---

## 4. What Is Verified Today

| Mitigation | Verified | Tracking |
|---|---|---|
| options_market collateral accounting (per-position) | ✅ | — |
| options_market pause/unpause (admin-gated) | ✅ | — |
| options_market `_via_multisig` alternatives (M-of-N, no delay) | ✅ | — |
| lp_pool epoch-queued deposits/withdrawals | ✅ | — |
| lp_pool utilization cap, series cap, idle-only collateral | ✅ | — |
| lp_token mint restricted to pinned pool | ✅ | — |
| multisig M-of-N approve/execute, signer rotation | ✅ | — |
| governor flash-vote resistance (snapshot voting power) | ✅ | — |
| vault per-tag escrow isolation | ❌ stub | #115 |
| price_oracle feeder-consensus price | ❌ stub | #115 |
| timelock admin delay | ❌ stub | #115 |
| governor queue/execute on-chain | ❌ stub | #115 |
| multisig timelock tiers (`is_executable`) | ❌ not built | #150 |
| multisig `__check_auth` custom account | ❌ not built | #150 |
| auto-expiring pauses | ❌ not built | #18 |
| rate-limited outflows | ❌ not built | #19 |
| granular access-control roles | ❌ not built | #17 |
