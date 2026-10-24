# Upgrade safety

An unguarded `upgrade(new_wasm_hash)` installs any code immediately. A
compromised or careless upgrade authority could take over a contract in
one transaction, and even an honest one has no safety net for a botched
release. `zenith_common::upgrade` adds two guards to every upgradeable
contract:

1. **Wasm hash allowlist with a delay.** A hash can only be installed
   after the upgrade authority registers it with
   `not_before = now + delay`. Registration emits
   `upgrade_proposed(wasm_hash, not_before)`, so users and monitors can
   fetch the code by hash and inspect it before it can land.
2. **Post-upgrade self-check.** Installing a wasm raises a guard that
   closes every user-facing entrypoint (`UpgradeCheckPending`, error 108)
   until anyone calls `post_upgrade_check()` on the new code and it passes.
   The check covers schema version, invariants and config sanity. If it
   fails, the contract stays closed. In effect it pauses itself until
   the failure is fixed.

Error codes 105–109 are shared by every upgradeable contract
([errors.md](errors.md)). The events are listed under "Shared framework
events" in [events-reference.md](events-reference.md).

## Procedure

```
propose ──(delay)──▶ upgrade ──▶ migrate (if the schema changed) ──▶ post_upgrade_check
   │                    │                                              │
   upgrade_proposed     upgraded: guard up                            upgrade_checked: guard down
                                                                       or upgrade_check_failed: stays up
```

1. **Propose.** The upgrade authority allowlists the hash. Watch for
   `upgrade_proposed`, then verify the wasm: build it from the tagged
   commit, compare hashes, and check the embedded metadata with
   `stellar contract info meta --wasm <file>` (`name`, `binver`,
   `source_repo`, `commit`; see the bindings section of the README).
   A proposal can be cancelled before it's installed (`upgrade_cancelled`).
2. **Install.** From `not_before` on, `upgrade(hash)` consumes the
   allowlist entry, raises the guard and swaps the wasm. Before
   `not_before` it fails with `UpgradeNotMatured` (106). For a hash that
   was never proposed, or was cancelled or already used, it fails with
   `WasmNotProposed` (105).
3. **Migrate.** If the new wasm raises `CURRENT_SCHEMA_VERSION`, run
   `migrate(old, new)` (and `migrate_batch` for paginated steps), as in
   [migrations.md](migrations.md). Upgrade, migrate and check entrypoints
   stay open while the guard is up.
4. **Check.** `post_upgrade_check()` is permissionless:
   - With a migration unfinished, it returns `MigrationInProgress` (102)
     and records nothing. Finish the migration first.
   - Otherwise it runs the contract's own invariants (multisig: a
     non-empty signer set, nonzero weights, valid thresholds, config
     present). It returns `true` and emits `upgrade_checked` if they hold,
     after which the contract is open again.
   - If an invariant fails, it returns `false`, records the error code in
     `get_upgrade_status()` (`failed`, `reason`) and emits
     `upgrade_check_failed`. It returns `false` rather than an error,
     because an error would revert the record. The guard stays up, and
     a later check can pass once the state is fixed, which in practice
     means installing a fixed wasm through the same allowlist.

Within one timelock or governance operation, steps 2–4 can run as
consecutive invocations: `[upgrade(hash), migrate(n-1, n),
post_upgrade_check()]`. Each is a separate invocation, so `migrate` and the
check run on the new code. Step 1 must be an earlier operation, at least
the delay before.

**Note: the old code enforces the guards.** Steps 1 and 2 run in the wasm
being replaced, so the allowlist and the guard protect upgrades away from
code that has them. The first upgrade from a wasm that predates this
framework installs without them.

## Emergency fixes

The allowlist accepts a shorter delay, down to `EMERGENCY_UPGRADE_DELAY`
(6 hours), only behind a **strictly higher approval threshold** than the
normal path. Such proposals carry `emergency: true` in `upgrade_proposed`,
so they stand out to monitors. The normal delay is `UPGRADE_DELAY`
(3 days).

| Contract | Normal path | Emergency path |
|---|---|---|
| multisig | Every signer approves the hash (`approve_upgrade`, N-of-N). The last approval allowlists it with `UPGRADE_DELAY`. | **None.** N-of-N is already the multisig's highest threshold, so there is nothing higher to justify a shorter delay. A botched multisig upgrade is fixed by unanimously approving a fixed wasm and waiting the full delay. The guard keeps the multisig closed in the meantime, so nothing can be approved or executed on the broken code. |
| options_market, price_oracle, vault | Admin, or the pinned multisig at `Critical` class, with `UPGRADE_DELAY`. | Intended: the pinned multisig, with an approval at a threshold above `Critical`. These contracts' sources are incomplete on `main` (README "Known gaps"). Their dormant `migrations.rs` still call the pre-allowlist API and must be moved onto `propose_wasm` / `install` / `post_upgrade_check` when the sources are restored. |

Any revoke of a multisig upgrade approval also removes the hash from the
allowlist. When unanimity is reached again, the delay restarts.

## Contracts

- **multisig** is the one upgradeable contract with complete sources on
  `main`, and is fully wired: `approve_upgrade`, `revoke_upgrade`,
  `upgrade`, `post_upgrade_check`, `get_upgrade_status`,
  `get_upgrade_ready_at`. Every state-changing entrypoint goes through
  `migrations::require_current`, which checks both the migration guard
  and the post-upgrade guard. Its `upgrade-wasm` tests
  (`cargo xtask test -p multisig`) run the whole flow against the real
  built wasm. They cover upgrading before maturity (rejected), after
  maturity (installed, then closed until checked), and a failing check
  (stays guarded).
- **params, governor, streams** have `migrate` but no `upgrade`
  entrypoint, so there is nothing to guard yet. Giving them one, and
  deciding its authority, is a separate change.
  **staking, grants_escrow, lp_pool, lp_token** are not upgradeable by
  design (see migrations.md, "Governance decisions").

## Allowlist storage

Each allowlist entry is a persistent entry keyed by the hash. Its TTL is
extended to cover the delay (assuming ledgers close no slower than every
3 s) plus a 30-day window to install, so an entry can't be archived
before it matures. The guard (`PendingCheck`) lives in instance storage
with the contract.
