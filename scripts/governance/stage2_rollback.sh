#!/usr/bin/env bash
# Rollback stage 2: the multisig (timelock proposer) schedules
# transfer_admin back to itself. Run with PHASE=schedule, then after
# TL_DELAY with PHASE=execute.
source "$(dirname "$0")/common.sh"
require MULTISIG_ACCOUNT PHASE

SALT=$(salt "rollback-stage2-$MARKET_ID")
ARG="[{\"address\":\"$MULTISIG_ACCOUNT\"}]"
case "$PHASE" in
  schedule)
    tl_schedule "$MULTISIG_ACCOUNT" \
      "[\"$MARKET_ID\",\"$ORACLE_ID\"]" '["transfer_admin","transfer_admin"]' "[$ARG,$ARG]" "$SALT"
    ;;
  execute)
    tl_execute "$MULTISIG_ACCOUNT" "$(tl_op_id \
      "[\"$MARKET_ID\",\"$ORACLE_ID\"]" '["transfer_admin","transfer_admin"]' "[$ARG,$ARG]" "$SALT")"
    show_admins
    ;;
esac

