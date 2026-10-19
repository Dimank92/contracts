# Governance Handbook

Zenith's admin powers (fees, listings, oracle config, pausing, upgrades) are
ultimately controlled by ZEN holders through the **governor** and executed by
the **timelock**. This handbook covers how that works and how to use it. The
path from today's single admin to this end state is in
[governance-handover.md](governance-handover.md).

## Contracts

| Contract   | Role |
|------------|------|
| `governor` | Proposals, voting, quorum. Queues passed proposals into the timelock. |
| `timelock` | Admin of every protocol contract. Delays each call by `min_delay`; anyone may execute a ready operation. Holds a proposer, an executor and a canceller role, and no admin of its own. |
| ZEN token  | Checkpointed voting token exposing `get_past_votes(account, ledger)` and `get_past_total_supply(ledger)`. The governor calls it through the `Votes` interface in `governor/src/interfaces.rs`. |

## Proposal lifecycle

```
propose ──► Pending ──(voting_delay)──► Active ──(voting_period)──► Defeated
                │                          │                        Succeeded ──queue──► Queued ──timelock.execute──► Executed
                └──────── cancel ──────────┴────────────────────────────┴───────────────────┘        └─(grace elapsed)─► Expired
                                         ▼
                                      Canceled
```

1. **Propose** — `propose(proposer, targets, fns, args, description_hash)`.
   The proposer needs at least `proposal_threshold` votes as of the previous
   ledger. The id is `sha256(xdr(targets, fns, args, description_hash))`
   (`hash_proposal` computes it off-chain too); an identical proposal cannot
   be submitted twice. At most `MAX_ACTIONS` (10) calls per proposal.
2. **Pending** until the snapshot ledger `propose_ledger + voting_delay`.
3. **Active** for `voting_period` ledgers after the snapshot. Vote with
   `cast_vote(voter, id, support)` where `support` is `0` Against, `1` For,
   `2` Abstain. Weight is the voter's power **at the snapshot ledger**.
   - `cast_vote_by_sig(voter, id, support)` is the gasless variant: the voter
     signs a Soroban authorization entry for exactly `(id, support)` and any
     relayer submits it. The host checks the signature and a nonce, so it
     can't be replayed.
4. **Succeeded / Defeated** — succeeded iff `for + abstain ≥ quorum` **and**
   `for > against`. A tie is a defeat. Quorum is
   `quorum_bps × past_total_supply(snapshot) / 10 000`.
5. **Queue** — anyone calls `queue(id)`; the governor schedules the batch in
   the timelock under the proposal's own id as the salt, with the timelock's
   `min_delay`. The timelock derives the operation id from the batch, the
   predecessor and the salt, so the governor derives the same id back whenever
   it needs to ask about the operation.
6. **Execute** — after the delay, anyone calls
   `timelock.execute(executor, targets, fns, args, predecessor, salt)`. The
   governor reports **Executed** once the timelock marks it done, or
   **Expired** if `grace_period` passes first.

### Cancelling

- The proposer may cancel while the proposal is **Pending**.
- **Anyone** may cancel a Pending/Active/Succeeded/Queued proposal if the
  proposer's current voting power has dropped below `proposal_threshold`.
  Cancelling a queued proposal also removes it from the timelock.
- The timelock's **canceller** can cancel any queued operation.

## Parameters

Changed only by a passed proposal targeting the governor itself (the call
arrives from the timelock). Ledgers assume ~5 s.

| Parameter            | Setter                    | Bounds |
|----------------------|---------------------------|--------|
| `voting_delay`       | `set_voting_delay`        | 0 – 241 920 ledgers (~2 weeks) |
| `voting_period`      | `set_voting_period`       | 720 (~1 h) – 241 920 ledgers |
| `quorum_bps`         | `set_quorum_bps`          | 100 (1%) – 10 000 (100%) |
| `proposal_threshold` | `set_proposal_threshold`  | ≥ 0 |

Timelock settings are changed by a proposal whose target is the timelock
itself: `update_delay(u64)`, `grant_role(Role, Address)`,
`revoke_role(Role, Address)` or `set_open_executor(bool)`. There is no instant
path — every change to the timelock waits out the delay like any other
operation.

## Attack resistance

| Attack | Mitigation |
|--------|------------|
| Flash-loan / last-minute votes | Power is read at the snapshot ledger, which is already in the past when voting opens. |
| Double voting | One vote per address per proposal (`has_voted`). Moving tokens after the snapshot does not create new power. |
| Proposal spam | `proposal_threshold`, and anyone can cancel a proposal whose proposer has sold below it. |
| Quorum manipulation | Quorum uses total supply at the snapshot, not at vote time. |
| Malicious proposal passes | `min_delay` gives users time to exit; before lock-in a canceller can cancel it. |

## Why the timelock executes, not the governor

Soroban forbids a contract from being re-entered. If the governor called the
timelock and the timelock called back into the governor (for example, to change
`quorum_bps`), the call would fail. The timelock's `execute` is therefore
open to anyone, and the governor never sits in the execution call stack.

