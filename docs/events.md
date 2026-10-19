# Events, schema v2

An indexer should be able to rebuild every contract's state from events
alone, without RPC reads. Reads break once entries are pruned or archived.
Schema v2 is designed for that. This page covers the conventions, the
design rules and the cut-over from v1. The full per-event list (topics,
data, state change) is [events-reference.md](events-reference.md),
generated from the machine-readable spec [events.json](../events.json).

## Conventions

- **Topics:** `(Symbol name, u32 schema_version, primary key...)`. The
  version is always the second topic, so a consumer can tell v1 from v2
  (and from any future v3) from the topics alone. Primary keys are the ids
  the event is about: `grant_id`, `stream_id`, `(action_id, signer)`,
  `(user, epoch)`, and so on.
- **Data:** a `#[contracttype]` struct named `<Event>Event`, encoded as an
  `ScMap` keyed by field name, never a positional tuple. Every field is
  named on the wire and listed in the contract spec, so the generated
  TypeScript bindings type it.
- **Framework events** from `zenith-common` (`schema_migrated`,
  `migration_progress`, `upgrade_*`) use the same layout. Each contract
  lists the ones it can emit under `shared_events` in events.json.
- **Standard events keep their standard.** lp_token's SEP-41 `approve`,
  `transfer`, `mint` and `burn` keep the SEP-41 layout (no version
  topic), because wallets and generic token indexers parse them. They're
  marked `"versioned": false` in events.json.

## Design rules

1. **Carry the resulting state, not only the delta.** If a call changes a
   counter or tally, the event carries the new value alongside the
   change: the registry `version` in params, `stake` and `total_staked`
   in staking, the vote tallies in `vote_cast`, the approval
   count and weight in `approved`, `idle` and `locked` in lp_pool. An
   indexer that missed nothing can check itself against these; one that
   starts mid-stream can sync from them.
2. **Creation events carry the whole object.** `grant_created` includes
   every milestone amount and deadline and the reviewer committee.
   `stream_created` includes the full schedule, `proposal_created` (governor and multisig)
   the full call payload, and `initialized` every config value.
3. **Every state transition has an event.** These were missing in v1 and
   now exist:
   - `initialized` on every contract.
   - staking `carryover_distributed`: rewards notified while nothing was
     staked used to reach the accumulator silently on the first stake.
   - lp_pool `keeper_set`, `max_utilization_set`, `series_cap_set` and
     `liabilities_marked` (admin and keeper setters emitted nothing).
   - multisig `proposal_created` (the executor payload), `vetoer_set` and
     `execute_by_set`.
   - grants_escrow `funds_reclaimed` now lists which milestones moved to
     `Reclaimed`, where v1 only gave the total.
4. **Time-driven transitions are derived, and documented.** Some state
   changes happen because the ledger advanced, not because of a call. The
   governor's `Pending` → `Active` → `Succeeded`/`Defeated`, a multisig
   approval going stale, and a stream accruing are examples. These have
   no event. Each event's `state` text in events.json says how to derive
   them from data already emitted: `snapshot`/`deadline`, tallies and
   `quorum(snapshot)`; `approved_at` and the TTL in `initialized`; the
   schedule in `stream_created`.
5. **Pure cleanup emits nothing.** multisig `prune` only deletes approval
   entries that can no longer affect `is_approved`, so it changes no
   indexed state.

## Machine-readable spec and its tests

[events.json](../events.json) is the source of truth. For each contract it
lists `types` (data structs and nested types), `events` (name, primary-key
topics, data type, and the state change in plain words), and
`shared_events`.

It's enforced from both directions, using the dev-only
[`tools/event-spec`](../tools/event-spec) crate:

- **Conformance.** Every contract's `test_events.rs` drives a scenario that
  emits each of its events, and `Recorder::record` checks every emitted
  event against the spec: name, version topic, topic count and types, and
  the data struct's exact field set and types. An event the spec doesn't
  describe fails the test.
- **Coverage.** `Recorder::assert_complete` fails if any event listed for
  the contract was never emitted, so the spec can't describe an event no
  test exercises. zenith-common's tests cover the shared events. The
  `upgraded` event needs a real wasm swap and is checked in multisig's
  `upgrade-wasm` tests.
- **Data content.** The same scenarios decode each event's data with
  `TryFromVal` into its struct and assert the field values, following
  the repo's existing convention.
- **Bindings.** `cargo xtask bindings` embeds events.json in the
  `@zenith/contracts` package (`eventSpec`, `decodeEvent`), and
  `cargo xtask events` generates [events-reference.md](events-reference.md).
  CI runs both with `--check`.

## Resource cost

Named struct data and the version topic make events larger. Events are
billed by size (`contract_events_size_bytes` in the transaction's
resource fee), so this is the price of indexer completeness. Measured as
the XDR size of topics plus data, for representative events:

| Event | v1 bytes | v2 bytes | Change |
|---|---:|---:|---:|
| `params.param_updated` | 104 | 212 | +108 (103%) |
| `streams.withdrawn` | 116 | 200 | +84 (72%) |
| `staking.staked` | 88 | 200 | +112 (127%) |
| `multisig.approved` | 148 | 312 | +164 (110%) |
| `governor.vote_cast` | 152 | 320 | +168 (110%) |
| `grants_escrow.grant_created` (2 milestones, 3 reviewers) | 160 | 604 | +444 (277%) |

Each event is a fraction of a kilobyte, far below the per-transaction
event size limit. The largest is `grant_created`, which grows linearly
with milestones and reviewers (bounded by `MAX_MILESTONES` and
`MAX_REVIEWERS`) and fires once per grant. Field names (the `ScMap` keys)
account for most of the increase. Fields were kept where they spare an
indexer a read, and dropped otherwise: `withdrawn` events don't repeat
the token, for example, because `stream_created` gave it once.

## Migration from v1: cut-over

v1 events are **not** emitted alongside v2. Each contract switches to v2
in the release that ships this change. Dual emission would have doubled
event cost for a release, and the version topic already makes the switch
unambiguous:

- A v2 event always has `u32 2` as its second topic. No v1 event had a
  `u32` second topic: v1 put a key (address, id, symbol) or nothing
  there. Indexers can therefore route by the second topic.
- Indexers should keep their v1 parsers for history and add v2 parsers
  keyed on `(name, 2)`. `decodeEvent` in the bindings returns `undefined`
  for anything that isn't v2, so it is safe to run over mixed history.
- Topic layouts changed. Keys moved behind the version topic, and some
  moved from topics into data: multisig `registered` now keys on
  `action_id` only, with `proposer` in the data, and
  `signer_change_vetoed` keys on `change_id`. Use the reference rather
  than shifting v1 offsets by one.
- A contract's events switch when its wasm is upgraded. The upgrade call
  runs the old code, so anything it emits (including its own `upgraded`
  event, if it has one) is still v1. Every event in later transactions
  is v2.
