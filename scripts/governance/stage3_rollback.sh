#!/usr/bin/env bash
# Rollback stage 3: the multisig is the timelock's canceller, so it can
# cancel the queued governor operation given as CANCEL_OP_ID. It keeps the
# proposer role until lock-in, so it can also schedule the corrective
# operation itself. After stage 4 it holds neither role and can do neither.
source "$(dirname "$0")/common.sh"
require MULTISIG_ACCOUNT

if [[ -n "${CANCEL_OP_ID:-}" ]]; then
  invoke "$TIMELOCK_ID" "$MULTISIG_ACCOUNT" cancel \
    --canceller "$MULTISIG_ACCOUNT" --id "$CANCEL_OP_ID"
fi
echo "timelock multisig is proposer:  $(view "$TIMELOCK_ID" has_role --role "$(role proposer)" --account "$MULTISIG_ACCOUNT")"
echo "timelock multisig is canceller: $(view "$TIMELOCK_ID" has_role --role "$(role canceller)" --account "$MULTISIG_ACCOUNT")"
