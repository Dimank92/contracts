# Vault

The `vault` crate is the protocol's collateral custody and accounting core. It holds
user deposits, tracks per-account balances and share accounting, and exposes the
admin surface used to configure the vault and route funds. It is the contract that
`lp_pool`, `options_market`, and the other strategy crates settle against.

> **Status:** This page documents the reconstructed `vault` contract. Every
> entrypoint below is verified against `vault/src/lib.rs`. Anything not yet
> implemented is marked **Pending** with a link to its tracking issue.

## Purpose

- Custody user collateral and protocol-owned funds.
- Maintain per-account balances and the vault's share accounting.
- Provide the admin/multisig surface for configuration and fund routing.
- Emit events that downstream indexers and the other crates consume.

## Entrypoints

All entrypoints are `pub fn` on the contract type. Auth requirements are noted per
entrypoint; "admin" means the address stored in `Admin` storage (see
[Admin / multisig path](#admin--multisig-path)).

### User-facing

| Entrypoint | Arguments | Auth | Notes |
| --- | --- | --- | --- |
| `deposit` | `env`, `from: Address`, `amount: i128` | `from` must authorize | Transfers `amount` from `from` into the vault and credits the account. |
| `withdraw` | `env`, `to: Address`, `amount: i128` | `to` must authorize | Debits the account and transfers `amount` to `to`. |
| `balance` | `env`, `account: Address` | none | Read-only; returns the account's balance. |

### Admin

| Entrypoint | Arguments | Auth | Notes |
| --- | --- | --- | --- |
| `initialize` | `env`, `admin: Address` | `admin` must authorize | One-time setup; stores the admin and initializes storage. |
| `set_admin` | `env`, `new_admin: Address` | admin | Rotates the admin address. |
| `pause` / `unpause` | `env` | admin | Toggles the paused flag that gates user-facing entrypoints. |

> **Pending:** Additional admin entrypoints (fee configuration, strategy routing)
> are not yet implemented in `vault/src/lib.rs`. Tracked in
> [#232](https://github.com/example/repo/issues/232).

## Storage layout summary

Storage keys are defined in `vault/src/storage.rs` (or the equivalent module in
`vault/src/lib.rs`). Summary:

| Key | Type | Description |
| --- | --- | --- |
| `Admin` | `Address` | Current admin / multisig address. |
| `Paused` | `bool` | When `true`, user-facing entrypoints revert. |
| `Balance(Address)` | `i128` | Per-account balance. |
| `TotalSupply` | `i128` | Aggregate tracked balance. |

See the repo-wide storage key layout document (tracked separately) for the
canonical key encoding.

## Error codes

Errors are defined in `vault/src/error.rs` and surfaced as `Result<_, Error>`.

| Code | Variant | Meaning |
| --- | --- | --- |
| 1 | `AlreadyInitialized` | `initialize` called more than once. |
| 2 | `NotInitialized` | Entrypoint called before `initialize`. |
| 3 | `Unauthorized` | Caller is not the admin or did not authorize. |
| 4 | `Paused` | User-facing entrypoint called while paused. |
| 5 | `InsufficientBalance` | Withdraw exceeds the account's balance. |
| 6 | `InvalidAmount` | Amount is zero or negative where not allowed. |

## Emitted events

Events are defined in `vault/src/events.rs` and emitted via `env.events().publish`.

| Event | Topics | Data | Emitted by |
| --- | --- | --- | --- |
| `deposit` | `("vault", "deposit", from)` | `amount: i128` | `deposit` |
| `withdraw` | `("vault", "withdraw", to)` | `amount: i128` | `withdraw` |
| `admin_changed` | `("vault", "admin")` | `new_admin: Address` | `set_admin` |
| `paused` / `unpaused` | `("vault", "paused")` | `bool` | `pause` / `unpause` |

## Admin / multisig path

The vault's admin is intended to be a `multisig` contract instance (see
[`docs/multisig.md`](./multisig.md)). The flow is:

1. Deploy the `multisig` contract and configure its signer set and threshold.
2. Deploy `vault` and call `initialize` with the multisig address as `admin`.
3. All admin-only entrypoints (`set_admin`, `pause`, `unpause`, and any future
   admin surface) must be invoked through the multisig, which enforces the
   threshold before forwarding the call.

Rotating the admin (`set_admin`) is itself an admin-only call and therefore must
go through the current multisig.

## Risk notes

See [`THREAT_MODEL.md`](../THREAT_MODEL.md) for the vault's threat model,
including reentrancy, pause-bypass, and admin-key-compromise considerations. This
page intentionally does not duplicate those notes.

## Related docs

- [`docs/lp_pool.md`](./lp_pool.md)
- [`docs/lp_token.md`](./lp_token.md)
- [`docs/multisig.md`](./multisig.md)
- [`docs/timelock.md`](./timelock.md)
- [`THREAT_MODEL.md`](../THREAT_MODEL.md)