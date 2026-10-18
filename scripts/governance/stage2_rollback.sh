#!/usr/bin/env bash
# Rollback stage 2: the multisig (timelock proposer) schedules
# transfer_admin back to itself through the timelock, then accepts.
#
#   PHASE=schedule  multisig: schedule transfer_admin(multisig) x2
#   PHASE=execute   anyone, after TL_DELAY
#   PHASE=accept    multisig: accept_admin on both contracts
source "$(dirname "$0")/common.sh"
require MULTISIG_ACCOUNT PHASE

ID=$(op_id "rollback-stage2-$MARKET_ID")
ARG="[{\"address\":\"$MULTISIG_ACCOUNT\"}]"
case "$PHASE" in
  schedule)
    tl_schedule "$MULTISIG_ACCOUNT" "$ID" \
      "[\"$MARKET_ID\",\"$ORACLE_ID\"]" '["transfer_admin","transfer_admin"]' "[$ARG,$ARG]"
    ;;
  execute)
    tl_execute "$MULTISIG_ACCOUNT" "$ID"
    show_admins
    ;;
  accept)
    accept_both "$MULTISIG_ACCOUNT"
    show_admins
    ;;
esac
