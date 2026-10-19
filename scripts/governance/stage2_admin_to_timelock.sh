#!/usr/bin/env bash
# Stage 1 → 2: the multisig account hands admin to the timelock (whose
# proposer and canceller are the multisig account).
# Rollback: stage2_rollback.sh.
source "$(dirname "$0")/common.sh"
require MULTISIG_ACCOUNT

echo "timelock proposer:  $(view "$TIMELOCK_ID" has_role --role "$(role proposer)" --account "$MULTISIG_ACCOUNT") (expect true)"
echo "timelock canceller: $(view "$TIMELOCK_ID" has_role --role "$(role canceller)" --account "$MULTISIG_ACCOUNT") (expect true)"
invoke "$MARKET_ID" "$MULTISIG_ACCOUNT" transfer_admin --new_admin "$TIMELOCK_ID"
invoke "$ORACLE_ID" "$MULTISIG_ACCOUNT" transfer_admin --new_admin "$TIMELOCK_ID"
show_admins

