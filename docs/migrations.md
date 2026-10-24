# Storage migrations

Every Zenith contract stores a `SchemaVersion(u32)` and exposes a
`migrate(from, to)` entrypoint. Any PR that changes a storage layout
ships its data migration as a **step** in this framework, instead of
writing its own lazy-migration logic.

The framework lives in `common/src/migrations.rs` (`zenith_common::migrations`).
Each crate's `src/migrations.rs` holds only its step registry and
`CURRENT_SCHEMA_VERSION`.

## Model

- **Layout versions.** Layout `1` is the baseline: storage as it was when
  this framework landed. `initialize` writes `CURRENT_SCHEMA_VERSION`. A
  contract deployed before the framework has no `SchemaVersion` key, and
  that reads as `1`.
- **Steps are code in the new wasm, keyed by the version they migrate
  *to*.** Step `n` turns a layout-`n-1` contract into a layout-`n` one.
  The wasm that introduces layout `n` also carries step `n` and sets
  `CURRENT_SCHEMA_VERSION = n`.
- **Upgrade, then migrate.** `upgrade(new_wasm_hash)` swaps the code.
  Then `migrate(n-1, n)` runs the step. In between, the stored version
  lags the code, and every state-changing entrypoint fails with
  `MigrationInProgress` (error 102). A half-migrated contract never
  serves traffic.

### What `migrate(from, to)` enforces

| Rule | Error |
|---|---|
| The stored version must equal `from`, so a completed step can't run again | `SchemaVersionMismatch` (100) |
| `from < to <= CURRENT_SCHEMA_VERSION` | `InvalidMigrationTarget` (101) |
| No paginated migration already in flight | `MigrationInProgress` (102) |

Steps `from+1 ..= to` run in order, and each one records its own version
bump and emits `schema_migrated(from, to)`. No step can be skipped, and
each runs exactly once.

### Pagination

A step over unbounded data (every position, every tag, every grant)
can't fit in one transaction. The step receives `(cursor, limit)`,
processes items `[cursor, cursor + limit)`, and returns
`Step::More(next_cursor)` until it finishes, then `Step::Done`.

- `migrate` runs each step with `DEFAULT_MIGRATION_BATCH` (50). If a step
  returns `More`, the migration is left **in progress**: the state is
  `(target, cursor)` and the event is `migration_progress(step, cursor)`.
- `migrate_batch(cursor, limit)` continues it. `cursor` must be exactly
  where the last call stopped, so pages can't be replayed or skipped.
  `limit` must be in `1..=MAX_MIGRATION_BATCH` (200). Errors:
  `NoMigrationInProgress` (103) and `InvalidMigrationBatch` (104).
- When the last step returns `Done`, the in-progress state is cleared and
  normal entrypoints unblock.
- `migrate_batch` is **permissionless**, like `bump`. The target was
  fixed by the authorized `migrate` call and the steps are fixed code, so
  a caller can only move the migration forward. It also means a
  governance-controlled contract never needs another proposal (which
  might itself be blocked) just to finish a migration.

Views: `schema_version()` and `migration_state() -> Option<(target, cursor)>`.

Framework storage (instance): `SchemaVersion`, plus `MigrationState` while
a migration is in flight. The keys belong to a private enum in
`zenith-common`, and no crate's `DataKey` uses those names.

## Who can call `migrate`

| Contract | `migrate` | Notes |
|---|---|---|
| options_market, price_oracle, vault | admin, or `migrate_via_multisig(action_id, from, to)` through the **pinned** multisig (`Emergency` class) | The steps were already timelocked as `Critical` with the upgrade that installed them. |
| multisig | any signer (`migrate(signer, from, to)`) | The steps are in a wasm every signer approved. |
| params, governor | timelock | Schedule it in the same timelock operation as the upgrade (see below). |
| streams | governance | |
| timelock | a delayed self-call `migrate(from, to)` dispatched by `execute` | The timelock is its own admin. |
| staking, grants_escrow | anyone | See "Governance decisions". |

## Upgrades

Every upgrade now goes through the wasm hash allowlist and the
post-upgrade check described in [upgrades.md](upgrades.md): propose the
hash, wait out the delay, `upgrade`, `migrate`, then `post_upgrade_check`.
`post_upgrade_check` requires the migration to be finished, so `migrate`
always comes first.

`upgrade(new_wasm_hash)` (admin) and `upgrade_via_multisig(action_id, new_wasm_hash)`
exist on options_market, price_oracle and vault. The shared implementation
is `zenith_common::upgrade`.

- **Trusted multisig.** `upgrade_via_multisig` does not take a
  `multisig_contract` argument. It only trusts the contract's trusted
  multisig, set by the admin with `set_multisig(multisig)`
  (`get_multisig()` to read) — the same multisig every `_via_multisig`
  entrypoint is restricted to.
- **Hash-bound approvals.** `action_id` (a `BytesN<32>`) must equal
  `upgrade_action_id(new_wasm_hash)` =
  `sha256("zenith.upgrade" || xdr(contract) || hash)`. Signers therefore
  approve one exact wasm for one exact contract, and like every approval
  it authorizes one call only. The action class is `Critical`.
- **multisig** upgrades itself with its own, stricter threshold. See
  below.

### Sequencing

A Soroban wasm swap takes effect once the invocation that performed it
returns. Therefore:

- **Via the timelock:** put `[target.upgrade(hash), target.migrate(n-1, n),
  target.post_upgrade_check()]` in **one** operation, after the hash's
  allowlist delay has passed. They are separate invocations of the
  target, so `migrate` and the check run the new code, and the upgrade
  is atomic with its migration.
- **Via admin or multisig:** submit `upgrade`, then `migrate`, as
  separate transactions. The contract is blocked (error 102) in between.
- **The timelock itself:** the upgrade and the `migrate` self-call must
  be separate operations.

## Authoring a step

1. Change the layout, and bump `CURRENT_SCHEMA_VERSION` in the crate's
   `src/migrations.rs` to `n`.
2. Add a step arm for `n`:

   ```rust
   pub fn step(env: &Env, to: u32, cursor: u32, limit: u32) -> Step {
       match to {
           2 => v2_split_escrow_key(env, cursor, limit),
           _ => panic_with_error!(env, Error::InvalidMigrationTarget),
       }
   }

   fn v2_split_escrow_key(env: &Env, cursor: u32, limit: u32) -> Step {
       let ids: Vec<u64> = /* the index you're walking */;
       let end = cursor.saturating_add(limit).min(ids.len());
       for i in cursor..end {
           // read old key, write new key, remove old key
       }
       if end == ids.len() { Step::Done } else { Step::More(end) }
   }
   ```

3. Rules for a step:
   - **Only move data.** No token transfers, no cross-contract calls, no
     auth checks: `migrate_batch` is permissionless.
   - **Bounded work per call.** Anything proportional to user data must
     honor `limit`. Walk an existing index. If there isn't one, add one
     in an earlier release.
   - **Always advance.** `More(c)` must have `c > cursor`, or the call
     fails with `InvalidMigrationBatch`.
   - **Don't reorder data the cursor walks** while the migration is in
     flight. The in-progress guard blocks the writers, and the step
     itself must not swap-remove from the index it is walking.
   - **Keep old steps.** A contract several versions behind runs all of
     them in order. Remove a step only when no deployment can still be
     below it, and say so in the PR.
   - **Extend TTLs** of the entries you write (`ttl::set_persistent`).
4. Call `migrations::require_current(&env)` at the top of every new
   state-changing entrypoint. Views stay available during a migration.
5. Test the upgrade path: register the old layout, run the steps, and
   assert both the new layout and that normal entrypoints unblock.
   `common/src/test.rs` is the reference harness: stale-schema blocking,
   ordering, no repeat or skip, and cursor/limit validation.

## Governance decisions

**Multisig self-upgrade needs every signer (N-of-N) plus a 3-day delay.**
The multisig's signer set, threshold and approval TTL are immutable, so
the M signers who run day-to-day actions can't rewrite who the signers
are. New wasm could change all three. If M-of-N could upgrade, M signers
could do indirectly what the design forbids directly. So an upgrade
requires `approve_upgrade(signer, hash)` from **every** signer. The last
approval starts `UPGRADE_DELAY` (3 days). Any single `revoke_upgrade`
cancels it. After the delay, anyone can call `upgrade(hash)`. The normal
`approve` / `execute` path can't be used, because Soroban forbids a
contract calling itself.

*Trade-off:* a single lost or uncooperative key blocks multisig upgrades
permanently. The recovery path is to deploy a new multisig and move each
contract's admin to it with the existing M-of-N
`transfer_admin_via_multisig`. That path needs only the operational
threshold, and all of it happens on-chain and in the open.

**staking and grants_escrow have no admin**, so their `migrate` is
permissionless. This is safe because neither contract can be upgraded.
With no upgrade entrypoint, no new steps can ever arrive, and `migrate`
can only fail. Adding an upgrade path to either one is a separate
decision, and it must add an authority for `migrate` at the same time.

**`migrate_via_multisig` is `Emergency` class**, with no additional
delay. The steps it runs were timelocked as `Critical` when the upgrade
that installed them was approved, and the contract is blocked until
they run.

## Status

governor, params, staking, streams, grants_escrow and multisig are fully
wired. multisig's `migrations.rs` and `upgrade.rs` are now part of the
crate, including the allowlist and the post-upgrade check.

The lib.rs of options_market, price_oracle, vault and timelock is still
empty or truncated on `main`: it was emptied by #148, #149 and #150,
together with several of those crates' `types.rs` / `error.rs`. Their
step registries and upgrade/migrate entrypoints are in their
`src/migrations.rs`, as separate `#[contractimpl]` blocks. Each file's
header lists the lines to add (`mod migrations;`, `init_schema` in
`initialize`, error variants) once those sources are restored. They
predate this PR's API changes too. Steps and guards now return `Result`,
and `upgrade::upgrade` is replaced by `require_auth` + `propose_wasm` +
`install` + `post_upgrade_check`, so the files must be ported with the
rest of each crate.
