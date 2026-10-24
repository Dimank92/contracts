#!/usr/bin/env bash
# Shared config for the governance handover scripts. See
# docs/governance-handover.md for what each stage does.
#
# Dry run is the DEFAULT: every command is printed, nothing is sent.
# Set DRY_RUN=0 to actually submit on the configured network.
set -euo pipefail

: "${NETWORK:=testnet}"
: "${DRY_RUN:=1}"

require() {
  for v in "$@"; do
    if [[ -z "${!v:-}" ]]; then
      echo "error: $v must be set" >&2
      exit 1
    fi
  done
}

# Contract ids and identities (stellar keys names or G.../C... addresses).
require MARKET_ID ORACLE_ID VAULT_ID TIMELOCK_ID GOVERNOR_ID

run() {
  if [[ "$DRY_RUN" == "1" ]]; then
    printf '[dry-run]'
    printf ' %q' "$@"
    printf '\n'
  else
    "$@"
  fi
}

# Calls made by the multisig account are only built (--build-only): the
# unsigned transaction XDR is printed for the M-of-N signers to sign with
# `stellar tx sign` and submit with `stellar tx send`.
invoke() {
  local id="$1" source="$2"
  shift 2
  local extra=()
  if [[ "$source" == "${MULTISIG_ACCOUNT:-}" ]]; then
    extra=(--build-only)
  fi
  run stellar contract invoke --network "$NETWORK" --source "$source" "${extra[@]}" \
    --id "$id" -- "$@"
}

# Read-only call; always executed so dry runs still show live state.
view() {
  local id="$1"
  shift
  if [[ "$DRY_RUN" == "1" ]] && ! command -v stellar >/dev/null; then
    echo "[dry-run] (stellar not installed) view $id $*"
    return
  fi
  stellar contract invoke --network "$NETWORK" --source "${VIEW_SOURCE:-${DEPLOYER:-}}" \
    --id "$id" --send=no -- "$@"
}

show_admins() {
  echo "options_market admin: $(view "$MARKET_ID" get_admin)"
  echo "price_oracle admin:   $(view "$ORACLE_ID" get_admin)"
  echo "vault admin:          $(view "$VAULT_ID" get_admin) (must stay options_market)"
}

# Schedules a timelock batch as $1 (the current proposer). Remaining args:
# targets_json fns_json args_json salt_hex.
#
# There is no operation id to pass in: the timelock derives it from the
# batch and the salt, and `schedule` returns it.
tl_schedule() {
  local proposer="$1" targets="$2" fns="$3" args="$4" salt="$5"
  invoke "$TIMELOCK_ID" "$proposer" schedule \
    --targets "$targets" --fns "$fns" --args "$args" \
    --salt "$salt" \
    --delay "${TL_DELAY:?TL_DELAY must be set}"
}

# The id of the operation a batch and a salt describe. The timelock derives
# it the same way, so this is what `execute` takes.
tl_op_id() {
  local targets="$1" fns="$2" args="$3" salt="$4"
  view "$TIMELOCK_ID" hash_operation \
    --targets "$targets" --fns "$fns" --args "$args" \
    --predecessor null --salt "$salt"
}

# Executes a ready operation. The id is the one `tl_schedule` returned, or
# the one `tl_op_id` computes.
tl_execute() {
  local executor="$1" id="$2"
  invoke "$TIMELOCK_ID" "$executor" execute \
    --executor "$executor" --id "$id"
}

# 32-byte hex salt derived from a label, so reruns are deterministic. The
# salt is what makes two otherwise identical batches distinct operations.
salt() {
  printf '%s' "$1" | sha256sum | cut -d' ' -f1
}

# The timelock's roles are a contract-type enum: Proposer = 0, Executor = 1,
# Canceller = 2.
role() {
  case "$1" in
    proposer) echo 0 ;;
    executor) echo 1 ;;
    canceller) echo 2 ;;
    *) echo "unknown role: $1" >&2; exit 1 ;;
  esac
}

