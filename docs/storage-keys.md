# Soroban Storage Key Layout

This document defines the storage key layout for every reconstructed contract
(`price_oracle`, `vault`, `timelock`, `options_market`) and the conventions that
prevent key collisions within a crate and accidental reuse of the same key
across logically distinct data.

Each contract centralizes its keys in a single `DataKey` enum. Where a crate
still defines keys inline, that is flagged below as follow-up consolidation
work — this document only establishes the convention and records the current
layout.

## Conventions

1. **One enum per contract.** All storage keys for a crate live in a single
   `DataKey` enum (or equivalent). Do not introduce ad-hoc keys outside it.
2. **Distinct logical data never share a key.** If two pieces of state can
   coexist, they must map to different variants. Composite keys (e.g.
   `(token, tag)` in `vault`, series id in `options_market`) must include every
   dimension that distinguishes the data.
3. **Reserved ranges.** Variants used for versioning/migration are reserved and
   must not be reused for business data. New schema keys are appended; existing
   variants are never renumbered or repurposed so the layout stays stable across
   upgrades.
4. **Instance vs persistent.** Small, contract-wide config belongs in instance
   storage; per-user/per-series accounting belongs in persistent storage. Each
   variant below records which it uses.
5. **Migration consistency.** Version/migration keys follow the `timelock`
   migration/version scheme (see `docs/migrations.md`).

## `price_oracle`

Keys are defined in `price_oracle/src/lib.rs`.

| Variant | Purpose | Storage |
| --- | --- | --- |
| `Admin` | Contract admin address | instance |
| `Asset` | Registered asset metadata | persistent |
| `Price` | Latest price per asset | persistent |
| `Version` | Schema version (reserved) | instance |

## `vault`

Keys are defined in `vault/src/types.rs`. Data is distinguished by
`(token, tag)`; every variant that stores per-token state must carry both
dimensions so distinct tags never collide.

| Variant | Purpose | Storage |
| --- | --- | --- |
| `Admin` | Contract admin address | instance |
| `Token(Token, Tag)` | Per-token, per-tag accounting | persistent |
| `TotalShares(Token, Tag)` | Aggregate shares per token/tag | persistent |
| `Version` | Schema version (reserved) | instance |

## `timelock`

Keys are defined in `timelock/src/types.rs` and used from `timelock/src/lib.rs`.
Versioning follows the migration scheme in `docs/migrations.md`.

| Variant | Purpose | Storage |
| --- | --- | --- |
| `Admin` | Contract admin address | instance |
| `Version` | Schema version (reserved) | instance |
| `Queued(u64)` | Queued operation by id | persistent |
| `NextId` | Monotonic operation id counter | instance |

## `options_market`

Keys are defined in `options_market/src/storage.rs`. Data is scoped by series
id; every per-series variant must include the series id.

| Variant | Purpose | Storage |
| --- | --- | --- |
| `Admin` | Contract admin address | instance |
| `Series(u64)` | Series metadata by id | persistent |
| `Position(u64, Address)` | Holder position within a series | persistent |
| `Version` | Schema version (reserved) | instance |

## Review checklist for adding a new key

- [ ] The key is a variant of the crate's single `DataKey` enum.
- [ ] The variant name and purpose are documented in this file.
- [ ] The variant cannot collide with any existing variant for logically
      distinct data (composite keys include every distinguishing dimension).
- [ ] The variant is not in the reserved version/migration range.
- [ ] Existing variants are unchanged (no renumbering or repurposing).
- [ ] Instance vs persistent storage is chosen deliberately and recorded.
- [ ] If the crate still defines keys inline, note the consolidation as
      follow-up rather than adding another inline key.
