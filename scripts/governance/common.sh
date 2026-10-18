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
  echo "options_market admin: $(view "$MARKET_ID" get_admin) (pending: $(view "$MARKET_ID" get_pending_admin))"
  echo "price_oracle admin:   $(view "$ORACLE_ID" get_admin) (pending: $(view "$ORACLE_ID" get_pending_admin))"
  echo "vault admin:          $(view "$VAULT_ID" get_admin) (must stay options_market)"
}

# Every _via_multisig path (including the short emergency-pause path that
# stays available once the timelock is admin) only accepts the contract's
# trusted multisig. Print it so a missing pin is caught before hand-off.
show_trusted_multisig() {
  echo "options_market trusted multisig: $(view "$MARKET_ID" get_multisig)"
  echo "price_oracle trusted multisig:   $(view "$ORACLE_ID" get_multisig)"
  echo "vault trusted multisig:          $(view "$VAULT_ID" get_multisig)"
}

# Step one of a two-step admin transfer on options_market and price_oracle,
# signed by the current admin $1. Nothing changes hands until step two.
propose_both() {
  invoke "$MARKET_ID" "$1" transfer_admin --new_admin "$2"
  invoke "$ORACLE_ID" "$1" transfer_admin --new_admin "$2"
}

# Step two, signed by the pending admin $1 (an account).
accept_both() {
  invoke "$MARKET_ID" "$1" accept_admin
  invoke "$ORACLE_ID" "$1" accept_admin
}

# Schedules a timelock batch as $1 (the current proposer). Remaining args:
# op_id targets_json fns_json args_json.
tl_schedule() {
  local proposer="$1" op_id="$2" targets="$3" fns="$4" args="$5"
  invoke "$TIMELOCK_ID" "$proposer" schedule \
    --op_id "$op_id" --targets "$targets" --fns "$fns" --args "$args" \
    --delay "${TL_DELAY:?TL_DELAY must be set}"
}

tl_execute() {
  invoke "$TIMELOCK_ID" "${EXECUTOR:-$1}" execute --op_id "$2"
}

# 32-byte hex op id derived from a label, so reruns are deterministic.
op_id() {
  printf '%s' "$1" | sha256sum | cut -d' ' -f1
}
