# Governance Handover Specification

How admin control of Zenith moves from a single deployer key, through a
multisig, to the timelock and governor. Every stage can be reversed by a
higher authority until the final lock-in.

Covered by `integration_tests/tests/handover.rs`, which deploys every contract
and runs each stage and each rollback, and `tests/timelock_admin.rs`, which
covers the timelock-held admin in depth. Scripts are in
`scripts/governance/`; the stage 1 → 2 runbook is
[runbooks/timelock-handoff.md](runbooks/timelock-handoff.md).

Every admin transfer is **two-step**: `transfer_admin(new)` records `new`
as pending, and control only moves when `new` calls `accept_admin()`. For
the timelock, that acceptance is itself a delayed timelock operation.

## Actors

- **Deployer**: the EOA that initialized the contracts.
- **Multisig account**: a native Stellar account with M-of-N signers (the
  same signer set as the on-chain `multisig` contract). It holds admin in
  stage 1 and is the timelock **proposer** and **guardian** from stage 2 on.
  A native account is used because the `multisig` contract only counts
  approvals and never makes calls, so it can't sign `admin.require_auth()`.
- **Multisig contract**: the trusted multisig (`set_multisig`) of
  options_market, price_oracle and vault; backs every `*_via_multisig`
  path, including the emergency pause that needs no timelock delay. It is
  not moved by the handover.
- **Timelock**: `proposer`, `guardian` (removed at lock-in), `min_delay`,
  `grace_period`.
- **Governor**: ZEN-holder voting. See [governance.md](governance.md).

## Stages

| Stage | options_market / price_oracle admin | Timelock proposer | Guardian | Who can roll back | How |
|------:|-------------------------------------|-------------------|----------|-------------------|-----|
| 0 | Deployer | — | — | — | — |
| 1 | Multisig account | — | — | Multisig | `transfer_admin(deployer)` + deployer `accept_admin`: `stage1_rollback.sh` |
| 2 | Timelock | Multisig account | Multisig account | Multisig | Schedules `transfer_admin(multisig)` through the timelock, then accepts: `stage2_rollback.sh`. It can also cancel any pending op. |
| 3 | Timelock | Governor | Multisig account | Guardian (multisig) | `cancel` + `guardian_set_proposer(multisig)`, which takes effect immediately: `stage3_rollback.sh` |
| 4 | Timelock | Governor | **none** | Governor only | Ordinary proposals. No emergency path. |

**Vault:** its admin is `options_market` at every stage and must stay that
way, because `options_market` moves escrow through it. At deployment the
deployer initializes it, sets its trusted multisig, `transfer_admin`s it to
options_market, and options_market calls `accept_vault_admin(vault)`. Its
only governance concern afterwards is the multisig recovery path
(`transfer_admin_via_multisig` + `accept_admin` by the recovery address).
The integration test checks the path still works after lock-in.

## Transitions

| Transition | Script | Mechanism |
|------------|--------|-----------|
| 0 → 1 | `stage1_admin_to_multisig.sh` | The deployer calls `transfer_admin(multisig_account)` on options_market and price_oracle; the multisig account calls `accept_admin` on both. |
| 1 → 2 | `stage2_admin_to_timelock.sh` (`PHASE=propose`, `schedule`, `execute`) | The multisig calls `transfer_admin(timelock)`, then schedules a timelock operation calling `accept_admin` on both; anyone executes it after `min_delay`. The script first checks the timelock's proposer, guardian and delay, and every contract's trusted multisig. |
| 2 → 3 | `stage3_proposer_to_governor.sh` | The multisig schedules a timelock self-call `set_prop(governor)`, then anyone executes it after `min_delay`. |
| 3 → 4 | `stage4_lock_in.sh` | A governor proposal calls the timelock's `lock_in()`. |

### Lock-in conditions

Run stage 4 only when all of the following are true:

1. The governor has run at least one real parameter-change proposal end to
   end in stage 3 (the integration test does this with `set_fee_rate` and
   `set_max_staleness`).
2. ZEN voting power is distributed widely enough that quorum can't be met by
   a single holder.
3. No queued timelock operation is pending review.
4. The multisig signers have signed off on removing their own guardian role.

After lock-in, `guardian_set_proposer` and guardian `cancel` fail for good.

## Running the scripts

Each script prints commands without sending them unless `DRY_RUN=0` is set.
Calls whose source is the multisig account run with `--build-only`. The
unsigned XDR is then signed by M signers (`stellar tx sign`) and submitted
(`stellar tx send`).

```bash
export NETWORK=testnet MARKET_ID=C... ORACLE_ID=C... VAULT_ID=C... \
       TIMELOCK_ID=C... GOVERNOR_ID=C... DEPLOYER=deployer \
       MULTISIG_ACCOUNT=G... TL_DELAY=172800

./scripts/governance/stage1_admin_to_multisig.sh
PHASE=propose  ./scripts/governance/stage2_admin_to_timelock.sh
PHASE=schedule ./scripts/governance/stage2_admin_to_timelock.sh
PHASE=execute  ./scripts/governance/stage2_admin_to_timelock.sh      # after TL_DELAY
PHASE=schedule ./scripts/governance/stage3_proposer_to_governor.sh
PHASE=execute  ./scripts/governance/stage3_proposer_to_governor.sh   # after TL_DELAY
PHASE=propose  PROPOSER=alice ./scripts/governance/stage4_lock_in.sh
```

Example dry-run output (stage 1 → 2, `PHASE=propose` then `schedule`):

```
[dry-run] stellar contract invoke --network testnet --source GMULTISIG --build-only --id CMARKET -- transfer_admin --new_admin CTIMELOCK
[dry-run] stellar contract invoke --network testnet --source GMULTISIG --build-only --id CORACLE -- transfer_admin --new_admin CTIMELOCK
[dry-run] stellar contract invoke --network testnet --source GMULTISIG --build-only --id CTIMELOCK -- schedule --op_id 3d7a…0b5e --targets ["CMARKET","CORACLE"] --fns ["accept_admin","accept_admin"] --args [[],[]] --delay 172800
```

## Emergency pause

Once the timelock is admin, every admin call waits `min_delay`, but
pausing must not: `pause_via_multisig(multisig, action_id)` on each
contract is authorized by the trusted multisig alone (`Emergency` class)
and lands immediately. Each approval authorizes one call. Unpausing is an
ordinary timelock operation.

## Trusted multisig

Every `*_via_multisig` entrypoint and `upgrade_via_multisig` only accept
the contract's trusted multisig, set by the admin with `set_multisig`
(earlier versions trusted any `multisig_contract` the caller named). Set
it on all three contracts before stage 2: afterwards it can only change
through the timelock.
