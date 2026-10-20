# Governance Handover Specification

How admin control of Zenith moves from a single deployer key, through a
multisig, to the timelock and governor. Every stage can be reversed by a
higher authority until the final lock-in.

Covered by `integration_tests/tests/handover.rs`, which deploys every contract
and runs each stage and each rollback. Scripts are in `scripts/governance/`.

## Actors

- **Deployer**: the EOA that initialized the contracts.
- **Multisig account**: a native Stellar account with M-of-N signers (the
  same signer set as the on-chain `multisig` contract). It holds admin in
  stage 1 and is the timelock **proposer** and **canceller** from stage 2 on.
  A native account is used because the `multisig` contract only counts
  approvals and never makes calls, so it can't sign `admin.require_auth()`.
- **Multisig contract**: backs every `*_via_multisig` recovery path. It is
  not moved by the handover.
- **Timelock**: `proposer`, `executor` and `canceller` role sets, `min_delay`
  and `grace_period`. The multisig's two roles are revoked at lock-in.
- **Governor**: ZEN-holder voting. See [governance.md](governance.md).

## Stages

| Stage | options_market / price_oracle admin | Timelock proposer | Timelock canceller | Who can roll back | How |
|------:|-------------------------------------|-------------------|--------------------|-------------------|-----|
| 0 | Deployer | — | — | — | — |
| 1 | Multisig account | — | — | Multisig | `transfer_admin(deployer)`: `stage1_rollback.sh` |
| 2 | Timelock | Multisig account | Multisig account | Multisig | Schedules `transfer_admin(multisig)` through the timelock: `stage2_rollback.sh`. It can also cancel any pending op. |
| 3 | Timelock | Multisig account **and** the governor | Multisig account | Multisig | Cancels the governor's queued op and schedules the corrective one: `stage3_rollback.sh`. |
| 4 | Timelock | Governor | **none** | Governor only | Ordinary proposals. No emergency path. |

Stage 3 *adds* the governor as a proposer rather than replacing the multisig:
the multisig keeps its proposer role until lock-in, which is what keeps the
stage reversible. A role can only be granted or revoked by a delayed
operation, so there is no instant path back to the multisig once the
governor holds the role alone.

**Vault:** its admin is `options_market` at every stage and must stay that
way, because `options_market` moves escrow through it. The vault's only
governance concern is its multisig recovery path
(`transfer_admin_via_multisig`), which takes the multisig contract per call.
There is no on-chain state to migrate. The integration test checks the path
still works after lock-in.

## Transitions

| Transition | Script | Mechanism |
|------------|--------|-----------|
| 0 → 1 | `stage1_admin_to_multisig.sh` | The deployer calls `transfer_admin(multisig_account)` on options_market and price_oracle. |
| 1 → 2 | `stage2_admin_to_timelock.sh` | The multisig calls `transfer_admin(timelock)`. First confirm that the timelock's proposer and canceller are the multisig account. |
| 2 → 3 | `stage3_proposer_to_governor.sh` | The multisig schedules a timelock self-call `grant_role(Proposer, governor)`, then anyone executes it after `min_delay`. |
| 3 → 4 | `stage4_lock_in.sh` | A governor proposal calls the timelock's `revoke_role(Proposer, multisig)` and `revoke_role(Canceller, multisig)`. |

### Lock-in conditions

Run stage 4 only when all of the following are true:

1. The governor has run at least one real parameter-change proposal end to
   end in stage 3 (the integration test does this with `set_fee_rate` and
   `set_max_staleness`).
2. ZEN voting power is distributed widely enough that quorum can't be met by
   a single holder.
3. No queued timelock operation is pending review.
4. The multisig signers have signed off on removing their own timelock roles.

After lock-in the multisig holds neither role, so it can neither cancel a
queued operation nor schedule one, for good.

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
./scripts/governance/stage2_admin_to_timelock.sh
PHASE=schedule ./scripts/governance/stage3_proposer_to_governor.sh
PHASE=execute  ./scripts/governance/stage3_proposer_to_governor.sh   # after TL_DELAY
PHASE=propose  PROPOSER=alice ./scripts/governance/stage4_lock_in.sh
```

Example dry-run output (stage 1 → 2):

```
[dry-run] stellar contract invoke --network testnet --source GMULTISIG --build-only --id CMARKET -- transfer_admin --new_admin CTIMELOCK
[dry-run] stellar contract invoke --network testnet --source GMULTISIG --build-only --id CORACLE -- transfer_admin --new_admin CTIMELOCK
```

## Known caveat

The `*_via_multisig` entrypoints on options_market, price_oracle and vault
accept any `multisig_contract` address from the caller. They don't pin one
at initialize, so they don't restrict who can act. This was already the case
before this handover and is out of scope here. It should be fixed, by pinning
the multisig at initialize, before mainnet.

