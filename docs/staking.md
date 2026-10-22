# Staking Contract

The `staking` crate lets token holders lock a balance, accrue rewards at a configured rate, and later unbond through a cooldown period. This page documents the interface as it exists in `staking/src/lib.rs`. Anything not yet implemented is marked **Pending** with a link to its tracking issue.

> **Status:** The `staking` crate is under active reconstruction and does not compile on `main` today. Sections below distinguish implemented entrypoints from pending ones. Risk notes live in [`THREAT_MODEL.md`](../THREAT_MODEL.md); they are not duplicated here.

## Purpose

- Custody staked balances per staker and per pool.
- Accrue rewards against a configurable reward rate.
- Enforce an unbonding cooldown before staked funds can be withdrawn.
- Give admins a bounded configuration surface (reward rate, cooldown) and an emergency control for incidents.

## Entrypoints

All state-mutating entrypoints require authorization via `common::require_admin_or_multisig` with the shared `Auth` enum unless noted otherwise. User-facing entrypoints require the staker's own authorization.

### User-facing

| Entrypoint | Arguments | Auth | Status |
| --- | --- | --- | --- |
| `stake` | `staker: Address`, `amount: i128` | `staker` | **Pending** — see [#234](https://github.com/org/repo/issues/234) |
| `unstake` | `staker: Address`, `amount: i128` | `staker` | **Pending** — see [#234](https://github.com/org/repo/issues/234) |
| `claim_rewards` | `staker: Address` | `staker` | **Pending** — see [#234](https://github.com/org/repo/issues/234) |
| `withdraw_unbonded` | `staker: Address` | `staker` | **Pending** — see [#234](https://github.com/org/repo/issues/234) |

### Queries

| Entrypoint | Arguments | Auth | Status |
| --- | --- | --- | --- |
| `get_config` | — | none | Implemented (see below) |
| `get_reward_rate` | — | none | Implemented (see below) |
| `get_cooldown` | — | none | Implemented (see below) |
| `get_stake` | `staker: Address` | none | **Pending** — see [#235](https://github.com/org/repo/issues/235) |
| `get_pending_rewards` | `staker: Address` | none | **Pending** — see [#235](https://github.com/org/repo/issues/235) |
| `get_unbond` | `staker: Address` | none | **Pending** — see [#235](https://github.com/org/repo/issues/235) |

### Admin

Admin entrypoints are authorized through `common::require_admin_or_multisig` using the shared `Auth` enum and the dedicated `AuthClass::Staking` class. Each mutation emits a versioned event.

| Entrypoint | Arguments | Auth | Status |
| --- | --- | --- | --- |
| `set_reward_rate` | `admin: Auth`, `rate: i128` | Admin or multisig | Implemented |
| `set_cooldown` | `admin: Auth`, `cooldown: u64` | Admin or multisig | Implemented |
| `set_emergency_mode` | `admin: Auth`, `mode: EmergencyMode` | Admin or multisig | Implemented |

#### `set_reward_rate(admin: Auth, rate: i128)`

Sets the reward rate used by the accrual path. Validation:

- `rate` must be `>= 0` and `<= MAX_REWARD_RATE`.
- A rate of `0` is allowed and explicitly disables accrual; it is not treated as an error.
- Out-of-range values are rejected with `StakingError::InvalidRewardRate`.
- The change does **not** retroactively re-price rewards already accrued. Accrual is settled against the previous rate before the new rate takes effect.

#### `set_cooldown(admin: Auth, cooldown: u64)`

Sets the unbonding cooldown, in seconds. Validation:

- `cooldown` must be `<= MAX_COOLDOWN`.
- Out-of-range values are rejected with `StakingError::InvalidCooldown`.
- The change applies only to unbonds initiated **after** the update. Existing unbond records keep the unlock time computed when they were created.

#### `set_emergency_mode(admin: Auth, mode: EmergencyMode)`

Toggles the emergency control. Semantics:

- `EmergencyMode::None` — normal operation.
- `EmergencyMode::BlockStakes` — new `stake` calls are rejected with `StakingError::StakesBlocked`; existing stakes, unbonds, and withdrawals are unaffected.
- `EmergencyMode::EmergencyExit` — stakers may exit immediately via `withdraw_unbonded`, bypassing the remaining cooldown. The exit path does **not** bypass any slashing penalty and does **not** permit double withdrawal; each unbond record is consumed exactly once.

The chosen semantics are covered by tests in `staking/src/test.rs`.

## Storage Layout Summary

Configuration is persisted with TTL extension per the workspace storage policy. All configuration reads go through a single accessor (`load_config`) so the accrual path and the admin path cannot disagree.

| Key | Type | Description |
| --- | --- | --- |
| `DataKey::Config` | `StakingConfig` | `{ reward_rate: i128, cooldown: u64, emergency_mode: EmergencyMode }` |
| `DataKey::Stake(Address)` | `i128` | Staked balance per staker — **Pending** ([#234](https://github.com/org/repo/issues/234)) |
| `DataKey::Unbond(Address)` | `UnbondRecord` | `{ amount: i128, unlock_at: u64 }` — **Pending** ([#234](https://github.com/org/repo/issues/234)) |
| `DataKey::Accrued(Address)` | `i128` | Accrued-but-unclaimed rewards — **Pending** ([#234](https://github.com/org/repo/issues/234)) |

## Error Codes

| Code | Variant | Meaning |
| --- | --- | --- |
| 1 | `Unauthorized` | Caller is not the required admin, multisig, or staker |
| 2 | `InvalidRewardRate` | Reward rate is negative or exceeds `MAX_REWARD_RATE` |
| 3 | `InvalidCooldown` | Cooldown exceeds `MAX_COOLDOWN` |
| 4 | `StakesBlocked` | New stakes rejected while emergency mode is `BlockStakes` |
| 5 | `InsufficientStake` | Unstake amount exceeds staked balance — **Pending** ([#234](https://github.com/org/repo/issues/234)) |
| 6 | `CooldownNotElapsed` | Withdrawal attempted before unlock time — **Pending** ([#234](https://github.com/org/repo/issues/234)) |
| 7 | `AlreadyWithdrawn` | Unbond record already consumed — **Pending** ([#234](https://github.com/org/repo/issues/234)) |

## Emitted Events

Events are versioned. Admin mutations emit the following:

| Event | Fields | Emitted by |
| --- | --- | --- |
| `reward_rate_set` | `rate: i128` | `set_reward_rate` |
| `cooldown_set` | `cooldown: u64` | `set_cooldown` |
| `emergency_mode_set` | `mode: EmergencyMode` | `set_emergency_mode` |

User-facing events (`staked`, `unstaked`, `rewards_claimed`, `unbonded`, `withdrawn`) are **Pending** — see [#234](https://github.com/org/repo/issues/234).

## Admin / Multisig Path

- The admin is stored and rotated through the shared two-step admin transfer in `common` (see the `common` crate docs).
- Every admin entrypoint accepts the shared `Auth` enum and is gated by `common::require_admin_or_multisig` with `AuthClass::Staking`.
- `Auth::Multisig` calls are validated against the multisig contract; integration coverage is tracked in the `Auth::Multisig` integration test issue.
- Configuration is read exclusively through `load_config`, so the accrual path and admin path observe the same values.

## Related

- [`docs/lp_pool.md`](./lp_pool.md) — reference shape for this page
- [`THREAT_MODEL.md`](../THREAT_MODEL.md) — risk notes
- Tracking issues: [#233](https://github.com/org/repo/issues/233) (admin controls), [#234](https://github.com/org/repo/issues/234) (user entrypoints), [#235](https://github.com/org/repo/issues/235) (query surface)