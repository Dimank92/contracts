# Runbook: hand the admin role to the timelock

How options_market, price_oracle (and any vault you administer directly)
end up with a live `timelock` deployment as their admin, so every admin
change is visible on-chain for at least `min_delay` before it can land.
This is stage 1 → 2 of the [governance handover](../governance-handover.md);
the end-to-end flow is exercised by
`integration_tests/tests/timelock_admin.rs` and `tests/handover.rs`.

The consumer contracts are agnostic: their admin is just an address. The
timelock satisfies their `admin.require_auth()` by being the direct
invoker of the calls it executes, so nothing in options_market,
price_oracle or vault changes when it takes over.

## Before you start

| Check | Command (view) | Expect |
|---|---|---|
| Timelock roles | `get_proposer`, `get_guardian` | the multisig account (both) |
| Timelock delays | `get_min_delay`, `get_grace_period` | the agreed `TL_DELAY` (e.g. 172800) and grace (e.g. 604800) |
| Emergency path | `get_multisig` on market, oracle, vault | the on-chain multisig contract |
| Current admins | `get_admin`, `get_pending_admin` | the multisig account, nothing pending |

The trusted multisig **must** be set on every contract before the
hand-off: once the timelock is admin, `set_multisig` is itself a delayed
operation, and `pause_via_multisig` (an `Emergency`-class approval of the
trusted multisig, no timelock delay) is what keeps the protocol pausable
during an incident. Unpausing goes through the timelock like any other
admin change.

## Steps

Both steps are needed because admin transfers are two-step everywhere:
`transfer_admin` only records a pending admin, and control moves when
that address calls `accept_admin`. For the timelock, the acceptance is a
delayed timelock operation — so the hand-off can only complete if the
timelock is live, correctly configured, and controlled by the expected
proposer.

```bash
export NETWORK=testnet MARKET_ID=C... ORACLE_ID=C... VAULT_ID=C... \
       TIMELOCK_ID=C... GOVERNOR_ID=C... MULTISIG_ACCOUNT=G... \
       TL_DELAY=172800 EXECUTOR=<any funded key>

# 1. Propose the timelock (multisig signs; prints unsigned XDR).
PHASE=propose  ./scripts/governance/stage2_admin_to_timelock.sh
# 2. Schedule the timelock's own accept_admin calls (multisig, as proposer).
PHASE=schedule ./scripts/governance/stage2_admin_to_timelock.sh
# 3. After TL_DELAY: anyone executes; the script prints the new admins.
PHASE=execute  ./scripts/governance/stage2_admin_to_timelock.sh
```

Every script prints its commands without sending them unless
`DRY_RUN=0`. Calls whose source is the multisig account are built with
`--build-only`; the M signers sign the XDR (`stellar tx sign`) and
submit it (`stellar tx send`).

A vault you administer directly (not the protocol vault, whose admin is
options_market) is handed off the same way: `transfer_admin(timelock)`
from its admin, then add `[VAULT_ID, "accept_admin", []]` to the
scheduled operation.

### Dry run

Against a local network (or with no `stellar` CLI at all) the scripts
print exactly what they would send:

```
$ NETWORK=local ... PHASE=propose ./scripts/governance/stage2_admin_to_timelock.sh
[dry-run] stellar contract invoke --network local --source GMULTISIG --build-only --id CMARKET -- transfer_admin --new_admin CTIMELOCK
[dry-run] stellar contract invoke --network local --source GMULTISIG --build-only --id CORACLE -- transfer_admin --new_admin CTIMELOCK
$ ... PHASE=schedule ...
[dry-run] stellar contract invoke --network local --source GMULTISIG --build-only --id CTIMELOCK -- schedule --op_id 3d7a…0b5e --targets ["CMARKET","CORACLE"] --fns ["accept_admin","accept_admin"] --args [[],[]] --delay 172800
$ ... PHASE=execute ...
[dry-run] stellar contract invoke --network local --source GMULTISIG --build-only --id CTIMELOCK -- execute --op_id 3d7a…0b5e
```

## Verify

- `get_admin` on market and oracle returns `TIMELOCK_ID`; `get_pending_admin` returns nothing.
- A direct admin call signed by the old admin (e.g. `set_fee_rate`) fails.
- A test operation (e.g. `set_fee_rate` to its current value) scheduled
  now fails `execute` with `NotReady` until `TL_DELAY` has passed.

## If something is wrong

| Situation | Recovery |
|---|---|
| Wrong `TIMELOCK_ID` proposed in step 1 | Nothing moved. Re-run `PHASE=propose` with the right id (it replaces the pending admin), or propose the multisig itself to cancel. |
| The timelock's proposer isn't the multisig | Step 2 fails (`schedule` needs the proposer's signature) and nothing moved. Deploy a correctly configured timelock and re-propose. |
| Step 3 fails (`NoPendingAdmin`) | Step 1 didn't land on one of the contracts; the whole operation reverted and nothing moved. Re-propose, schedule a fresh operation. |
| The timelock is admin but must be rolled back | `stage2_rollback.sh` (schedule `transfer_admin(multisig)`, execute after the delay, then the multisig accepts). |
| A malicious or mistaken operation is pending | Any canceller — the proposer, the guardian until lock-in, or an address granted with a `set_canc` self-call — calls `cancel(caller, op_id)`. |
| An incident needs an immediate pause | Signers approve an action id on the multisig contract; anyone submits `pause_via_multisig(multisig, action_id)` on each contract. Each approval works once. |

## Ordering and clocks

- `schedule_after(op_id, predecessor, ...)` forces ordering, e.g. an
  `upgrade` before the `migrate` that depends on it. The predecessor must
  already exist, so dependencies can't form a cycle; cancelling a
  predecessor only strands its own dependents (cancel those too).
- Every delay is measured on the ledger **timestamp**, the same clock the
  governor uses for proposal `eta`s — never on ledger counts.
