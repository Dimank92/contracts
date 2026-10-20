#!/usr/bin/env bash
# Stage 2 → 3: the multisig schedules a timelock self-call adding the
# governor as a proposer. Run with PHASE=schedule, then after TL_DELAY with
# PHASE=execute. Rollback: stage3_rollback.sh (the multisig keeps its
# proposer role until lock-in, so it can cancel and re-propose).
source "$(dirname "$0")/common.sh"
require MULTISIG_ACCOUNT PHASE

SALT=$(salt "stage3-$TIMELOCK_ID")
TARGETS="[\"$TIMELOCK_ID\"]"
FNS='["grant_role"]'
ARGS="[[$(role proposer),\"$GOVERNOR_ID\"]]"
case "$PHASE" in
  schedule)
    tl_schedule "$MULTISIG_ACCOUNT" "$TARGETS" "$FNS" "$ARGS" "$SALT"
    ;;
  execute)
    tl_execute "$MULTISIG_ACCOUNT" "$(tl_op_id "$TARGETS" "$FNS" "$ARGS" "$SALT")"
    echo "timelock governor is proposer: $(view "$TIMELOCK_ID" has_role --role "$(role proposer)" --account "$GOVERNOR_ID") (expect true)"
    ;;
esac

