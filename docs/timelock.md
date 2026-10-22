# Timelock

The `timelock` crate provides a governance-controlled delay mechanism for
privileged operations. Operations are queued, held for a configurable delay,
and only then may be executed by an authorized executor. This gives token
holders and integrators time to react to upcoming admin actions.

Source: `timelock/src/lib.rs`.

## Purpose

- Queue privileged operations with an ETA (earliest time of execution).
- Enforce a minimum delay between queueing and execution.
- Allow the admin to cancel queued operations before execution.
- Provide a public, queryable record of pending operations.

## Entrypoints

All entrypoints follow the Soroban contract convention: the first argument is
the caller address (`env.invoker()` / `Address` parameter) and must be
authorized via `require_auth()` where noted.

| Function | Arguments | Auth | Description |
| --- | --- | --- | --- |
| `initialize` | `admin: Address`, `delay: u64` | `admin` | One-time setup. Stores the admin and the minimum delay (in ledgers or seconds, per the crate's time unit). Fails if already initialized. |
| `queue` | `caller: Address`, `id: BytesN<32>`, `eta: u64` | `admin` | Schedules an operation identified by `id` for execution no earlier than `eta`. Rejects `eta` earlier than `now + delay`. Rejects duplicate `id`. |
| `execute` | `caller: Address`, `id: BytesN<32>` | `admin` | Executes a queued operation. Fails if `id` is unknown or if `now < eta`. Removes the operation from storage on success. |
| `cancel` | `caller: Address`, `id: BytesN<32>` | `admin` | Cancels a queued operation. Fails if `id` is unknown. |
| `get_eta` | `id: BytesN<32>` | none | Returns the ETA for a queued operation, or `None` if not queued. |
| `get_delay` | — | none | Returns the configured minimum delay. |
| `get_admin` | — | none | Returns the current admin. |

> **Pending:** The exact set of privileged operations that may be queued
> (i.e. the payload carried alongside `id`) is not yet finalized in the
> reconstructed source. Tracking issue: see the `timelock` reconstruction
> task in the project issue tracker.

## Storage Layout

| Key | Type | Description |
| --- | --- | --- |
| `DataKey::Admin` | `Address` | The admin authorized to queue, execute, and cancel. |
| `DataKey::Delay` | `u64` | Minimum delay between queueing and execution. |
| `DataKey::Operation(id)` | `u64` (ETA) | ETA for each queued operation, keyed by `id`. |

Storage is instance storage for `Admin` and `Delay`; per-operation entries use
persistent storage so they survive ledger archival within the delay window.

## Error Codes

| Code | Name | Meaning |
| --- | --- | --- |
| 1 | `AlreadyInitialized` | `initialize` called more than once. |
| 2 | `NotInitialized` | Entrypoint called before `initialize`. |
| 3 | `Unauthorized` | Caller is not the admin. |
| 4 | `EtaInPast` | `eta` is earlier than `now + delay`. |
| 5 | `OperationExists` | `id` is already queued. |
| 6 | `OperationNotFound` | `id` is not queued. |
| 7 | `NotReady` | `execute` called before the operation's ETA. |

## Emitted Events

| Event | Topics | Data | When |
| --- | --- | --- | --- |
| `queued` | `("timelock", "queued")` | `(id, eta)` | On successful `queue`. |
| `executed` | `("timelock", "executed")` | `id` | On successful `execute`. |
| `cancelled` | `("timelock", "cancelled")` | `id` | On successful `cancel`. |

## Admin / Multisig Path

The `admin` is the sole authority for `queue`, `execute`, and `cancel`. In
production the admin should be the `multisig` contract (see
[`docs/multisig.md`](./multisig.md)) so that no single key can schedule or
execute privileged operations. The timelock's delay is the second line of
defense: even a compromised multisig cannot act instantly.

For risk analysis of the admin path, see [`THREAT_MODEL.md`](../THREAT_MODEL.md).
</｜｜DSML｜｜ parameter>
</｜｜DSML｜｜ invoke>
</｜｜DSML｜｜ calls>