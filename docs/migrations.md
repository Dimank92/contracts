# Storage Migrations

This document describes the versioned storage scheme used by the `timelock`
crate and the procedure for upgrading a deployed timelock contract.

## Why a versioned layout

The timelock is intended to hold the admin role on every other contract in the
system. If its storage layout changes between releases and an upgrade silently
reinterprets old records, role assignments (proposer / executor / canceller),
predecessor dependencies, and pending or cancelled operations can be lost. That
would remove the delayed-execution exit window the crate exists to provide.

To prevent this, the timelock stores an explicit schema version marker and
exposes an admin-gated `migrate` entrypoint that upgrades old records to the
current layout in a bounded, resumable way.

## Version marker

The current schema version is stored under a `DataKey`-based key in instance
storage (e.g. `DataKey::SchemaVersion`). A freshly initialized contract writes
the current version, so a new deployment is already at the latest schema.

A shared helper (e.g. `ensure_schema_version` / `current_schema_version`) is
called at entry to read the marker and decide whether a migration is required.

## `migrate` entrypoint

`migrate` is gated to the timelock admin. It:

1. Reads the stored schema version.
2. If it already equals the current version, returns immediately (no-op).
3. Otherwise walks the old keys and rewrites them into the current layout,
   extending TTL on every migrated key.
4. Writes the new schema version marker last, so a partially-applied migration
   can be resumed by calling `migrate` again.

### Guarantees

- **Idempotent.** Calling `migrate` twice is a no-op; the second call sees the
  current version and returns without touching storage.
- **Non-destructive.** Role assignments, predecessor dependencies, and
  pending/cancelled operation state are preserved across the upgrade.
- **Resumable.** Because the version marker is written only after all records
  are upgraded, an interrupted migration can be re-run safely.
- **Fresh contracts.** Migrating a freshly initialized contract is a no-op
  because initialization already writes the current version.

## Upgrade procedure

1. Deploy the new timelock wasm.
2. As the timelock admin, invoke `migrate`.
3. Verify the schema version marker now reports the current version.
4. Confirm proposer / executor / canceller roles and any pending operations are
   unchanged.

If step 2 is interrupted, simply invoke `migrate` again; it resumes from the
last consistent state.
