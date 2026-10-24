# Reentrancy guard (`common::guard`) and staking commitments

## Guard / CEI
`common::guard::{enter, exit, with_guard}` keeps a flag in instance storage per key; re-entry panics
with the caller's own error. A failed invocation rolls back storage, so a panicking token cannot
strand the flag. CEI order: acquire -> validate -> mutate -> external call -> release.
The vault and options_market entrypoints are not yet implemented in this tree; apply `with_guard`
to every escrow/settlement path when they land.

## Staking commitments (feeder bonds)
`staking.commit(user, committer, key, amount)` locks active stake under an opaque `(contract, key)`
slot (per-slot, not global); it cannot be unstaked. `start_release` (committer) begins the cooldown;
`finalize_release` is permissionless once it elapses. `slash(...)` (committer) takes from unbonding
stake first, then active stake; amounts above the commitment are rejected; each incident id can be
slashed once per committer. Not yet wired: the `price_oracle` side (its source is absent) and the
fail-closed quarantine on a failed slash cross-call.
