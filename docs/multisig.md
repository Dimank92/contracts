# `multisig` Contract

The `multisig` crate implements an M-of-N signer authorization contract. It is the
canonical admin path for protocol contracts that require multi-party approval
before privileged operations (upgrades, parameter changes, treasury moves).

This page documents the interface **as implemented** in `multisig/src/lib.rs`.
Anything not yet implemented is marked **Pending** with a link to its tracking
issue. For risk notes, see [`THREAT_MODEL.md`](../THREAT_MODEL.md); this page
does not duplicate them.

---

## Purpose

- Hold a set of authorized signers and an approval threshold (`M` of `N`).
- Let any authorized signer propose a transaction (a target contract + method +
  arguments).
- Let authorized signers approve or revoke approval on a pending proposal.
- Execute a proposal once it has accumulated at least `M` approvals.
- Expose read-only introspection so off-chain tooling and other contracts can
  discover signers, threshold, and proposal state without guessing storage keys.

The contract is intentionally narrow: it does not interpret the payload it
executes. Authorization is the only thing it enforces.

---

## Entrypoints

All entrypoints are invoked through the Soroban contract interface. Auth
requirements are stated per entrypoint. "Signer auth" means the caller must be
one of the configured signers and must have authorized the invocation
(`require_auth`).

### Initialization

#### `initialize(env, signers: Vec<Address>, threshold: u32)`

- **Auth:** none (one-shot initializer; reverts if already initialized).
- **Behavior:** stores the signer set and threshold. Rejects an empty signer
  set, a threshold of `0`, or a threshold greater than the number of signers.
- **Events:** emits `init` (see [Events](#events)).

### Proposals

#### `propose(env, proposer: Address, target: Address, method: Symbol, args: Vec<Val>) -> u64`

- **Auth:** signer auth on `proposer`.
- **Behavior:** creates a pending proposal and records the proposer's approval.
  Returns the new proposal id.
- **Events:** emits `propose`.

#### `approve(env, signer: Address, proposal_id: u64)`

- **Auth:** signer auth on `signer`.
- **Behavior:** records `signer`'s approval on the proposal. Idempotent per
  signer; approving twice does not double-count. Reverts if the proposal is not
  pending or does not exist.
- **Events:** emits `approve`.

#### `revoke(env, signer: Address, proposal_id: u64)`

- **Auth:** signer auth on `signer`.
- **Behavior:** removes `signer`'s approval. Reverts if the proposal is not
  pending or does not exist. Revoking an approval that was never given is a
  no-op.
- **Events:** emits `revoke`.

#### `execute(env, executor: Address, proposal_id: u64)`

- **Auth:** signer auth on `executor`.
- **Behavior:** requires the proposal to be pending and to have at least
  `threshold` approvals. Marks the proposal executed and invokes the target with
  the stored method and arguments. Reverts if the threshold is not met or the
  proposal is not pending.
- **Events:** emits `execute`.

### Signer management

#### `add_signer(env, caller: Address, signer: Address)`

- **Auth:** signer auth on `caller`; the call must itself be routed through the
  multisig (i.e. executed via `execute`) so that adding a signer requires the
  configured threshold. Direct invocation by a single signer is rejected.
- **Behavior:** adds `signer` to the set. Reverts if already present.
- **Events:** emits `signer_added`.

#### `remove_signer(env, caller: Address, signer: Address)`

- **Auth:** same as `add_signer`.
- **Behavior:** removes `signer`. Reverts if absent, or if removal would drop
  the signer count below the current threshold.
- **Events:** emits `signer_removed`.

#### `set_threshold(env, caller: Address, threshold: u32)`

- **Auth:** same as `add_signer`.
- **Behavior:** updates the approval threshold. Rejects `0` or a value greater
  than the current signer count.
- **Events:** emits `threshold_set`.

### Query / introspection

These entrypoints were added in #231 so that tooling and dependent contracts can
read multisig state without hard-coding storage keys.

#### `get_signers(env) -> Vec<Address>`

- **Auth:** none.
- **Behavior:** returns the current signer set. Enumeration is **bounded** by
  the configured maximum signer count (see [Storage](#storage-layout-summary));
  the contract never returns an unbounded list. Callers should treat the result
  as the complete, ordered set.

#### `get_threshold(env) -> u32`

- **Auth:** none.
- **Behavior:** returns the current approval threshold.

#### `get_proposal(env, proposal_id: u64) -> Proposal`

- **Auth:** none.
- **Behavior:** returns the stored proposal record (target, method, args,
  approvals, status). Reverts if the id does not exist.

#### `get_approvals(env, proposal_id: u64) -> Vec<Address>`

- **Auth:** none.
- **Behavior:** returns the addresses that have approved the proposal. Bounded
  by the signer count.

#### `is_signer(env, address: Address) -> bool`

- **Auth:** none.
- **Behavior:** returns whether `address` is a current signer.

### Pending

- **`get_proposal_count(env) -> u64`** — total number of proposals ever created.
  *Pending; tracked in #231.*
- **`get_pending_proposals(env) -> Vec<u64>`** — ids of proposals still pending.
  *Pending; tracked in #231.*
- **`cancel(env, signer: Address, proposal_id: u64)`** — cancel a pending
  proposal before execution. *Pending; tracked in #231.*

---

## Storage Layout Summary

Storage is organized under a small set of typed keys. Exact key encodings live
in the repo-wide storage key layout document (out of scope here); this section
records the shape.

| Key (logical)        | Type                          | Notes                                              |
|----------------------|-------------------------------|----------------------------------------------------|
| `Signers`            | `Vec<Address>`                | Ordered set; bounded by `MAX_SIGNERS`.             |
| `Threshold`          | `u32`                         | `1 <= threshold <= signers.len()`.                 |
| `ProposalCount`      | `u64`                         | Monotonic counter used to allocate proposal ids.   |
| `Proposal(id)`       | `Proposal`                    | Per-proposal record.                               |
| `Approval(id, addr)` | `bool`                        | Presence marks an approval; enables O(1) checks.   |

`Proposal` contains: `target: Address`, `method: Symbol`, `args: Vec<Val>`,
`proposer: Address`, `status: ProposalStatus` (`Pending` | `Executed`).

**Bounded signer enumeration:** `Signers` is capped at `MAX_SIGNERS`. Both
`get_signers` and `get_approvals` therefore return lists of bounded length, and
`add_signer` reverts once the cap is reached. This keeps introspection cheap and
prevents unbounded iteration in `execute`'s threshold check.

---

## Error Codes

Errors are returned as contract errors. Codes are stable and must not be
renumbered.

| Code | Name                  | Meaning                                                        |
|------|-----------------------|----------------------------------------------------------------|
| 1    | `AlreadyInitialized`  | `initialize` called more than once.                            |
| 2    | `NotInitialized`      | Entrypoint called before `initialize`.                         |
| 3    | `InvalidThreshold`    | Threshold is `0` or exceeds the signer count.                  |
| 4    | `EmptySigners`        | `initialize` called with no signers.                           |
| 5    | `NotSigner`           | Caller is not a configured signer.                             |
| 6    | `SignerExists`        | `add_signer` for an address already present.                   |
| 7    | `SignerNotFound`      | `remove_signer` for an address not present.                    |
| 8    | `TooManySigners`      | `add_signer` would exceed `MAX_SIGNERS`.                       |
| 9    | `ThresholdTooHigh`    | `remove_signer` would drop below the current threshold.        |
| 10   | `ProposalNotFound`    | No proposal with the given id.                                 |
| 11   | `ProposalNotPending`  | Proposal is already executed (or cancelled, once supported).   |
| 12   | `ThresholdNotMet`     | `execute` called with fewer than `threshold` approvals.        |
| 13   | `DirectAdminCall`     | Signer-management entrypoint invoked outside `execute`.        |

---

## Events

Events follow the standard `(topics, data)` shape. Topic `0` is the event name.

| Event            | Topics                          | Data                                  |
|------------------|---------------------------------|---------------------------------------|
| `init`           | `("init",)`                     | `(signers, threshold)`                |
| `propose`        | `("propose", proposer)`         | `(proposal_id, target, method)`       |
| `approve`        | `("approve", signer)`           | `(proposal_id,)`                      |
| `revoke`         | `("revoke", signer)`            | `(proposal_id,)`                      |
| `execute`        | `("execute", executor)`         | `(proposal_id,)`                      |
| `signer_added`   | `("signer_added", signer)`      | `()`                                  |
| `signer_removed` | `("signer_removed", signer)`    | `()`                                  |
| `threshold_set`  | `("threshold_set",)`            | `(threshold,)`                        |

---

## Admin / Multisig Path

The multisig contract is the admin path for dependent contracts. The intended
flow is:

1. A signer calls `propose` with the target contract, method, and arguments.
2. Other signers call `approve` until the approval count reaches `threshold`.
3. Any signer calls `execute`, which invokes the target.

Signer-management operations (`add_signer`, `remove_signer`, `set_threshold`)
must themselves be routed through `propose` → `approve` → `execute`; a direct
call by a single signer is rejected with `DirectAdminCall`. This means changing
the signer set or threshold requires the same M-of-N approval as any other
privileged action.

Dependent contracts should authorize privileged entrypoints against the
multisig contract address (via `require_auth` on the multisig), not against
individual signer addresses. Use `get_signers` / `get_threshold` /
`is_signer` for off-chain verification and for cross-contract checks that need
to confirm the current configuration.

For the trust assumptions and failure modes of this design, see
[`THREAT_MODEL.md`](../THREAT_MODEL.md).

---

## Related

- [`docs/lp_pool.md`](./lp_pool.md) — reference for this page's structure.
- [`docs/lp_token.md`](./lp_token.md)
- [`THREAT_MODEL.md`](../THREAT_MODEL.md)
- Tracking issue for pending entrypoints: #231.