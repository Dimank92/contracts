# `price_oracle`

The `price_oracle` crate provides on-chain price feeds consumed by the rest of the
protocol (notably `options_market` and `staking`). It stores per-asset price
records that are pushed by an authorized oracle account and exposes read-only
accessors for downstream contracts.

This page documents the interface **as implemented** in `price_oracle/src/lib.rs`.
Anything that is not yet implemented is called out explicitly under
[Pending](#pending) with a link to its tracking issue.

> Risk notes (oracle manipulation, staleness, key management) live in
> [`THREAT_MODEL.md`](../THREAT_MODEL.md). This page does not duplicate them.

## Purpose

- Hold the latest price for each supported asset.
- Record the timestamp of the last update so consumers can enforce staleness
  bounds.
- Restrict writes to an authorized oracle identity, configured by the admin.

## Entrypoints

All entrypoints are `pub fn` on the contract `impl`. Auth requirements are
stated per entrypoint.

### `initialize`

```rust
pub fn initialize(env: Env, admin: Address, oracle: Address) -> Result<(), Error>
```

- **Arguments**
  - `admin`: address that may rotate the oracle and perform admin actions.
  - `oracle`: address authorized to push prices.
- **Auth**: `admin.require_auth()`.
- **Behavior**: writes the admin and oracle identities into instance storage.
  Fails with `Error::AlreadyInitialized` if called more than once.

### `set_oracle`

```rust
pub fn set_oracle(env: Env, new_oracle: Address) -> Result<(), Error>
```

- **Arguments**
  - `new_oracle`: replacement oracle identity.
- **Auth**: `admin.require_auth()` (admin read from instance storage).
- **Behavior**: rotates the authorized oracle. Fails with
  `Error::NotInitialized` if the contract has not been initialized.

### `set_price`

```rust
pub fn set_price(env: Env, asset: Symbol, price: i128, timestamp: u64) -> Result<(), Error>
```

- **Arguments**
  - `asset`: asset identifier.
  - `price`: price in the oracle's fixed-point representation (see
    [Storage layout](#storage-layout)).
  - `timestamp`: ledger timestamp the price is valid as of.
- **Auth**: `oracle.require_auth()` (oracle read from instance storage).
- **Behavior**: stores the price and timestamp for `asset` in persistent
  storage. Rejects non-positive prices with `Error::InvalidPrice`. Fails with
  `Error::NotInitialized` if the contract has not been initialized.

### `get_price`

```rust
pub fn get_price(env: Env, asset: Symbol) -> Result<PriceRecord, Error>
```

- **Arguments**
  - `asset`: asset identifier.
- **Auth**: none (read-only).
- **Behavior**: returns the stored `PriceRecord { price, timestamp }`. Fails
  with `Error::PriceNotFound` if no price has been recorded for `asset`.

### `get_admin` / `get_oracle`

```rust
pub fn get_admin(env: Env) -> Result<Address, Error>
pub fn get_oracle(env: Env) -> Result<Address, Error>
```

- **Auth**: none (read-only).
- **Behavior**: return the configured admin / oracle. Fail with
  `Error::NotInitialized` if the contract has not been initialized.

## Storage layout

| Key                 | Storage kind | Value          | Written by        |
| ------------------- | ------------ | -------------- | ----------------- |
| `DataKey::Admin`    | Instance     | `Address`      | `initialize`      |
| `DataKey::Oracle`   | Instance     | `Address`      | `initialize`, `set_oracle` |
| `DataKey::Price(Symbol)` | Persistent | `PriceRecord` | `set_price` |

`PriceRecord` is a struct with `price: i128` and `timestamp: u64`. Prices are
stored as `i128` in the oracle's fixed-point representation; consumers are
responsible for scaling. Persistent price entries have their TTL extended on
each `set_price` call.

## Error codes

| Variant              | Code | Meaning                                              |
| -------------------- | ---- | ---------------------------------------------------- |
| `AlreadyInitialized` | 1    | `initialize` called on an already-initialized contract |
| `NotInitialized`     | 2    | Entrypoint requires initialization but it has not run |
| `InvalidPrice`       | 3    | `set_price` received a non-positive price            |
| `PriceNotFound`      | 4    | `get_price` for an asset with no recorded price      |

## Emitted events

| Event           | Topics                    | Data                          | Emitted by   |
| --------------- | ------------------------- | ----------------------------- | ------------ |
| `price_updated` | `("price", asset)`        | `(price: i128, timestamp: u64)` | `set_price`  |
| `oracle_set`    | `("oracle",)`             | `new_oracle: Address`         | `set_oracle` |

Event names are the `Symbol` values passed to `env.events().publish(...)`.

## Admin / multisig path

- The `admin` identity is set once at `initialize` and is the only address
  permitted to call `set_oracle`.
- In production deployments the `admin` is expected to be the protocol
  [multisig](./multisig.md) rather than a single key. See
  [`docs/multisig.md`](./multisig.md) for the proposal/approval flow.
- The `oracle` identity is a hot key used by the price-pushing service; it can
  be rotated by the admin via `set_oracle` without redeploying the contract.

## Pending

The following items are referenced by the README but are **not** implemented in
`price_oracle/src/lib.rs` as of this writing. They are tracked separately and
must not be relied upon:

- **Multi-asset batch updates** (`set_prices`) — pending, see
  [#232](https://github.com/example/repo/issues/232).
- **Staleness enforcement on read** (`get_price` returning an error past a
  configured max age) — pending, see
  [#232](https://github.com/example/repo/issues/232).
- **Per-asset decimals metadata** — pending, see
  [#232](https://github.com/example/repo/issues/232).

## See also

- [`docs/lp_pool.md`](./lp_pool.md) — doc structure this page follows.
- [`docs/multisig.md`](./multisig.md) — admin/multisig flow.
- [`THREAT_MODEL.md`](../THREAT_MODEL.md) — oracle risk notes.