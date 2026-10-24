# Grant program operations guide

How to run a grant or bounty program on `grants_escrow`. Contract
reference: [README](../README.md#grants_escrow-reference).

## Roles

| Role | Does | Must not |
|---|---|---|
| Funder (treasury) | Creates and funds grants; reclaims missed milestones. | — |
| Grantee | Submits one evidence hash per milestone before its deadline. | Be one of the grant's reviewers (rejected at creation). |
| Reviewers | Approve submitted milestones. `quorum` of them release a milestone. | Approve the same milestone twice (rejected). |

The reviewer committee can be individual accounts or multisig contract
addresses — any `Address` that can `require_auth`.

## Lifecycle

1. **Scope the grant off-chain.** Agree the milestone list: amount and
   deadline (unix seconds) for each. Payouts are all-or-nothing per
   milestone, so split work that should pay out in parts into separate
   milestones.
2. **Create and fund.** The funder calls `create_grant(funder, grantee,
   token, milestones, reviewers, quorum)`. The full total transfers
   upfront, so the grantee can verify the funds exist before starting.
   Record the returned `grant_id`.
3. **Submit.** For each milestone the grantee hosts evidence off-chain
   (PR links, reports, ...), hashes it (SHA-256), and calls
   `submit_milestone(grant_id, idx, hash)` before the deadline.
4. **Review.** Reviewers fetch the evidence, check its hash against
   `get_grant(grant_id).milestones[idx].evidence_hash`, and each call
   `approve_milestone`. The approval that reaches `quorum` transfers the
   milestone amount to the grantee in the same transaction.
5. **Missed deadlines.** A milestone that is not released by
   `deadline + GRACE_PERIOD` (7 days) — whether never submitted or
   submitted but never approved — can be returned to the funder with
   `reclaim(grant_id)`. Later milestones stay claimable. Reviewers
   should finish reviews inside the grace period.

## Monitoring

Index these events: `grant_created`, `milestone_submitted`,
`milestone_approved`, `milestone_released`, `funds_reclaimed`,
`dispute_opened`, `dispute_resolved`, `refunded`. Grantee
dashboards page through `get_grants_by_grantee(grantee, start, limit)`
(max 50 per page) with `get_grantee_grant_count` for the total.

## Disputes

An arbiter is set once with `initialize(arbiter)`. While a milestone is
`Submitted`, the funder or the grantee may call `open_dispute(grant_id,
idx, caller)`; it becomes `Disputed`, can no longer be approved, and is
excluded from `reclaim` (its funds are held for the arbiter). A second
open reverts with `DisputeAlreadyOpen`. Anyone else reverts with
`NotDisputeParty`.

The arbiter (or an executable multisig action, via
`resolve_dispute_via_multisig`) calls `resolve_dispute(grant_id, idx,
outcome, split_bps)`, with no deadline:

| Outcome | Effect |
|---|---|
| `Release` | Whole amount to the grantee; milestone `Released`. |
| `Reject` | Whole amount back to the funder; milestone `Refunded`. The grant's other milestones continue unchanged. |
| `Split` | `split_bps` (1..=9 999) to the grantee, rounded down; the remainder to the funder. Milestone `Released`. |

`split_bps` must be 0 for `Release`/`Reject`. If the arbiter is the
grant's funder or grantee, resolution reverts with `ArbiterConflict`.
Events: `dispute_opened`, `dispute_resolved`, `refunded`; read state with
`get_dispute(grant_id, idx)` and the milestone `status` in `get_grant`.
Escrowed funds always equal the sum of `Pending`, `Submitted` and
`Disputed` milestone amounts.

## Keeping grants live

Long grants must not archive. Grant data is extended on every read and
write; for idle grants call `bump([Grant(id), GranteeGrants(grantee)])`
periodically, or restore per the [archival runbook](runbooks/archival.md).
