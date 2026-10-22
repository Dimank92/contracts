#!/usr/bin/env bash
#
# scripts/settlement-keeper.sh
#
# Thin wrapper around the `settlement-keeper` binary (tools/settlement-keeper)
# for running permissionless expiry settlement against a network.
#
# Mirrors the dry-run-by-default convention used by scripts/governance/common.sh:
# unless explicitly told otherwise, this wrapper runs the keeper in --dry-run
# mode so an operator can inspect the intended settlement calls and the
# observed oracle state before anything is submitted on-chain.
#
# Usage:
#   scripts/settlement-keeper.sh [--once|--poll] [--dry-run|--submit] [-- <keeper flags>]
#
# Environment:
#   NETWORK              Network to target (default: testnet). One of:
#                        local | standalone | futurenet | testnet | mainnet
#   SETTLEMENT_KEEPER_BIN
#                        Path to the keeper binary. Defaults to
#                        target/release/settlement-keeper, falling back to
#                        target/debug/settlement-keeper, then to
#                        `cargo run -p settlement-keeper --`.
#   SETTLEMENT_KEEPER_KEY
#                        Optional secret key (S...) passed through to the
#                        keeper. Prefer the keeper's own env/flag handling;
#                        this wrapper never writes key material to disk or logs.
#
# Examples:
#   scripts/settlement-keeper.sh --once
#   scripts/settlement-keeper.sh --once --submit
#   NETWORK=local scripts/settlement-keeper.sh --poll --submit
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------

# Network defaults to testnet; override with NETWORK=... in the environment.
NETWORK="${NETWORK:-testnet}"

# Dry-run by default. Operators must opt in to submitting transactions.
DRY_RUN=1

# Mode: "once" or "poll". Defaults to a single pass so a stray invocation
# never starts a long-running loop by accident.
MODE="once"

# Extra flags forwarded verbatim to the keeper binary.
FORWARD_ARGS=()

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

usage() {
  cat <<'EOF'
Usage: scripts/settlement-keeper.sh [options] [-- <keeper flags>]

Options:
  --once            Run a single pass over expiring/expired series (default).
  --poll            Run continuously, polling for expiring series.
  --dry-run         Print intended settlement calls and oracle state only (default).
  --submit          Actually submit settlement transactions.
  --network <name>  Target network: local|standalone|futurenet|testnet|mainnet.
                    Overrides the NETWORK environment variable.
  -h, --help        Show this help and exit.

Any arguments after `--` are forwarded verbatim to the settlement-keeper binary.

Environment:
  NETWORK                 Target network (default: testnet).
  SETTLEMENT_KEEPER_BIN   Path to the keeper binary (optional).
  SETTLEMENT_KEEPER_KEY   Optional secret key passed through to the keeper.

By default this wrapper runs in --dry-run mode. Pass --submit to send
transactions on-chain.
EOF
}

die() {
  echo "settlement-keeper: $*" >&2
  exit 1
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

while [[ $# -gt 0 ]]; do
  case "$1" in
    --once)
      MODE="once"
      shift
      ;;
    --poll)
      MODE="poll"
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --submit)
      DRY_RUN=0
      shift
      ;;
    --network)
      [[ $# -ge 2 ]] || die "--network requires a value"
      NETWORK="$2"
      shift 2
      ;;
    --network=*)
      NETWORK="${1#--network=}"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      FORWARD_ARGS+=("$@")
      break
      ;;
    *)
      # Unknown flags are forwarded to the keeper so this wrapper does not
      # have to track every keeper option.
      FORWARD_ARGS+=("$1")
      shift
      ;;
  esac
done

# ---------------------------------------------------------------------------
# Validate network
# ---------------------------------------------------------------------------

case "${NETWORK}" in
  local|standalone|futurenet|testnet|mainnet)
    ;;
  *)
    die "unknown NETWORK '${NETWORK}' (expected: local|standalone|futurenet|testnet|mainnet)"
    ;;
esac

# ---------------------------------------------------------------------------
# Resolve the keeper binary
# ---------------------------------------------------------------------------

resolve_keeper() {
  if [[ -n "${SETTLEMENT_KEEPER_BIN:-}" ]]; then
    [[ -x "${SETTLEMENT_KEEPER_BIN}" ]] \
      || die "SETTLEMENT_KEEPER_BIN='${SETTLEMENT_KEEPER_BIN}' is not executable"
    printf '%s\n' "${SETTLEMENT_KEEPER_BIN}"
    return 0
  fi

  local candidate
  for candidate in \
    "${REPO_ROOT}/target/release/settlement-keeper" \
    "${REPO_ROOT}/target/debug/settlement-keeper"; do
    if [[ -x "${candidate}" ]]; then
      printf '%s\n' "${candidate}"
      return 0
    fi
  done

  # Fall back to cargo run so a fresh checkout works without a manual build.
  if command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "cargo-run"
    return 0
  fi

  die "could not find settlement-keeper binary; build it with 'cargo build -p settlement-keeper' or set SETTLEMENT_KEEPER_BIN"
}

KEEPER="$(resolve_keeper)"

# ---------------------------------------------------------------------------
# Assemble keeper arguments
# ---------------------------------------------------------------------------

KEEPER_ARGS=()
KEEPER_ARGS+=("--network" "${NETWORK}")

if [[ "${MODE}" == "poll" ]]; then
  KEEPER_ARGS+=("--poll")
else
  KEEPER_ARGS+=("--once")
fi

if [[ "${DRY_RUN}" -eq 1 ]]; then
  KEEPER_ARGS+=("--dry-run")
else
  KEEPER_ARGS+=("--submit")
fi

# Forward any remaining flags.
if [[ ${#FORWARD_ARGS[@]} -gt 0 ]]; then
  KEEPER_ARGS+=("${FORWARD_ARGS[@]}")
fi

# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------

# Summarize the invocation without ever echoing key material.
if [[ "${DRY_RUN}" -eq 1 ]]; then
  echo "settlement-keeper: network=${NETWORK} mode=${MODE} dry-run=yes" >&2
else
  echo "settlement-keeper: network=${NETWORK} mode=${MODE} dry-run=no" >&2
fi

if [[ "${KEEPER}" == "cargo-run" ]]; then
  exec cargo run --quiet --manifest-path "${REPO_ROOT}/Cargo.toml" \
    -p settlement-keeper -- "${KEEPER_ARGS[@]}"
else
  exec "${KEEPER}" "${KEEPER_ARGS[@]}"
fi