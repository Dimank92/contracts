#!/usr/bin/env bash
# Stage 3 → 4: a governor proposal revoking the multisig's timelock roles,
# which permanently removes the rollback paths. IRREVERSIBLE.
# PHASE=propose → vote (VOTER, SUPPORT=1) → queue → execute.
source "$(dirname "$0")/common.sh"
require PHASE

DESC=$(salt "Lock in governance handover")
TARGETS="[\"$TIMELOCK_ID\",\"$TIMELOCK_ID\"]"
FNS='["revoke_role","revoke_role"]'
ARGS="[[$(role proposer),\"$MULTISIG_ACCOUNT\"],[$(role canceller),\"$MULTISIG_ACCOUNT\"]]"
case "$PHASE" in
  propose)
    require PROPOSER
    invoke "$GOVERNOR_ID" "$PROPOSER" propose --proposer "$PROPOSER" \
      --targets "$TARGETS" --fns "$FNS" --args "$ARGS" --description_hash "$DESC"
    ;;
  vote)
    require VOTER PROPOSAL_ID
    invoke "$GOVERNOR_ID" "$VOTER" cast_vote --voter "$VOTER" \
      --id "$PROPOSAL_ID" --support "${SUPPORT:-1}"
    ;;
  queue)
    require PROPOSAL_ID EXECUTOR
    invoke "$GOVERNOR_ID" "$EXECUTOR" queue --id "$PROPOSAL_ID"
    ;;
  execute)
    require PROPOSAL_ID EXECUTOR
    tl_execute "$EXECUTOR" "$(tl_op_id "$TARGETS" "$FNS" "$ARGS" "$PROPOSAL_ID")"
    echo "locked in: $(view "$TIMELOCK_ID" has_role --role "$(role proposer)" --account "$MULTISIG_ACCOUNT") (expect false)"
    ;;
esac

