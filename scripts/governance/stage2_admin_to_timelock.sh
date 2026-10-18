#!/usr/bin/env bash
# Stage 1 → 2: the multisig account hands admin of options_market and
# price_oracle to the timelock (whose proposer and guardian are the
# multisig account). Two-step, and the second step is itself a delayed
# timelock operation — so the hand-off only completes if the timelock is
# live, correctly configured, and controlled by the multisig.
#
#   PHASE=propose   multisig: transfer_admin(timelock) on both contracts
#   PHASE=schedule  multisig (timelock proposer): schedule accept_admin x2
#   PHASE=execute   anyone, after TL_DELAY: execute it, then verify
#
# Until PHASE=execute succeeds the multisig is still admin; a wrong
# TIMELOCK_ID is recovered by re-running PHASE=propose with the right one
# (or proposing the multisig itself, which cancels the transfer).
# Rollback after execute: stage2_rollback.sh.
source "$(dirname "$0")/common.sh"
require MULTISIG_ACCOUNT PHASE

ID=$(op_id "handoff-stage2-$MARKET_ID-$ORACLE_ID-$TIMELOCK_ID")
case "$PHASE" in
  propose)
    echo "timelock proposer: $(view "$TIMELOCK_ID" get_proposer) (expect $MULTISIG_ACCOUNT)"
    echo "timelock guardian: $(view "$TIMELOCK_ID" get_guardian) (expect $MULTISIG_ACCOUNT)"
    echo "timelock delay:    $(view "$TIMELOCK_ID" get_min_delay) (expect $TL_DELAY)"
    show_trusted_multisig
    propose_both "$MULTISIG_ACCOUNT" "$TIMELOCK_ID"
    show_admins
    ;;
  schedule)
    show_admins # both must show pending: $TIMELOCK_ID
    tl_schedule "$MULTISIG_ACCOUNT" "$ID" \
      "[\"$MARKET_ID\",\"$ORACLE_ID\"]" '["accept_admin","accept_admin"]' '[[],[]]'
    echo "op_id: $ID — executable after TL_DELAY=$TL_DELAY seconds"
    ;;
  execute)
    echo "ready: $(view "$TIMELOCK_ID" is_ready --op_id "$ID")"
    tl_execute "$MULTISIG_ACCOUNT" "$ID"
    show_admins # both must show admin: $TIMELOCK_ID, pending: none
    ;;
  *)
    echo "error: PHASE must be propose, schedule or execute" >&2
    exit 1
    ;;
esac
