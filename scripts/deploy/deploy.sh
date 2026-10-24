#!/usr/bin/env bash
# scripts/deploy/deploy.sh -- deploy, initialize and wire the full Zenith
# contract stack to a Soroban testnet.
#
# What this script does (in order):
#   1.  Validates prerequisites (stellar CLI, funded account, wasm on disk).
#   2.  Deploys multisig, price_oracle, timelock, vault, options_market.
#   3.  Initializes each contract (skips with a clear message if already done).
#   4.  Wires cross-contract references:
#         - price_oracle: add FEEDER_1 (and FEEDER_2 if set) as feeders
#         - vault:        add options_market as an authorized integrator and
#                         allow the collateral token
#         - options_market: set price_oracle as the oracle
#         - pin multisig on price_oracle so *_via_multisig entrypoints work
#   5.  Writes all contract addresses to addresses.json.
#
# Secrets are read from the environment, NEVER written to disk or logs.
#
# Required environment variables:
#   DEPLOYER          stellar-cli identity name (key name in ~/.config/stellar)
#   COLLATERAL_TOKEN  Stellar asset contract address (e.g. USDC testnet SAC)
#   FEE_RECIPIENT     address that receives protocol fees
#
# Optional environment variables:
#   FEEDER_1          first oracle price feeder address (default: DEPLOYER address)
#   FEEDER_2          second oracle feeder address (optional)
#   NETWORK           stellar-cli network name (default: testnet)
#   ADDRESSES_FILE    where to write addresses.json (default: scripts/deploy/addresses.json)
#   TIMELOCK_DELAY    minimum delay in seconds for timelock (default: 3600 = 1h)
#   MULTISIG_THRESHOLD  M-of-N threshold (default: 1)
#   MULTISIG_TTL      approval TTL in seconds; 0 = never expires (default: 0)
#   WASM_DIR          path to compiled wasm (default: target/wasm32-unknown-unknown/release)
#
# Usage:
#   export DEPLOYER=alice
#   export COLLATERAL_TOKEN=C...USDC...
#   export FEE_RECIPIENT=G...
#   scripts/deploy/deploy.sh
#   scripts/deploy/deploy.sh --dry-run    # print commands without sending
#
# Re-running after a partial failure is safe: already-initialized contracts
# are detected and skipped. Already-deployed contracts re-use the address
# from addresses.json to avoid orphaned deployments.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WASM_DIR="${WASM_DIR:-${REPO_ROOT}/target/wasm32-unknown-unknown/release}"
NETWORK="${NETWORK:-testnet}"
ADDRESSES_FILE="${ADDRESSES_FILE:-${REPO_ROOT}/scripts/deploy/addresses.json}"
TIMELOCK_DELAY="${TIMELOCK_DELAY:-3600}"
MULTISIG_THRESHOLD="${MULTISIG_THRESHOLD:-1}"
MULTISIG_TTL="${MULTISIG_TTL:-0}"
DRY_RUN=0

# -- argument parsing ----------------------------------------------------------
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --help|-h)
      sed -n 's/^# \{0,2\}//p' "$0" | head -50
      exit 0
      ;;
    *) echo "error: unknown argument '$arg'" >&2; exit 1 ;;
  esac
done

# -- helpers -------------------------------------------------------------------
info()  { echo "[deploy] $*"; }
ok()    { echo "  [ok] $*"; }
warn()  { echo "  [warn] $*"; }
fail()  { echo "  [fail] error: $*" >&2; exit 1; }

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    fail "'$1' not found -- $2"
  fi
}

# -- prerequisites -------------------------------------------------------------
need stellar "install stellar-cli: https://developers.stellar.org/docs/tools/cli/stellar-cli"
need jq      "install jq: https://stedolan.github.io/jq/"

# Required env vars
: "${DEPLOYER:?DEPLOYER must be set to a stellar-cli identity name}"
: "${COLLATERAL_TOKEN:?COLLATERAL_TOKEN must be set to the collateral token contract address}"
: "${FEE_RECIPIENT:?FEE_RECIPIENT must be set}"

# FEEDER_1 defaults to the deployer's own address
if [ -z "${FEEDER_1:-}" ]; then
  FEEDER_1="$(stellar keys address "${DEPLOYER}" --network "${NETWORK}" 2>/dev/null)" \
    || fail "Could not resolve address for identity '${DEPLOYER}'. Is it funded on ${NETWORK}?"
  info "FEEDER_1 defaulting to deployer address: ${FEEDER_1}"
fi

# -- wasm files ----------------------------------------------------------------
require_wasm() {
  local path="$1"
  if [ ! -f "${path}" ]; then
    fail "Wasm not found: ${path}\nRun scripts/deploy/build.sh first."
  fi
}
require_wasm "${WASM_DIR}/zenith_multisig.wasm"
require_wasm "${WASM_DIR}/zenith_price_oracle.wasm"
require_wasm "${WASM_DIR}/zenith_timelock.wasm"
require_wasm "${WASM_DIR}/zenith_vault.wasm"
require_wasm "${WASM_DIR}/zenith_options_market.wasm"

# -- dry-run wrapper -----------------------------------------------------------
run() {
  if [ "${DRY_RUN}" -eq 1 ]; then
    printf '[dry-run]'
    printf ' %q' "$@"
    printf '\n'
  else
    "$@"
  fi
}

# -- addresses.json helpers ----------------------------------------------------
load_addresses() {
  if [ -f "${ADDRESSES_FILE}" ]; then
    info "Loading existing addresses from ${ADDRESSES_FILE}"
  else
    echo '{}' > "${ADDRESSES_FILE}"
  fi
}

get_address() {
  jq -r --arg k "$1" '.[$k] // empty' "${ADDRESSES_FILE}"
}

save_address() {
  local key="$1" value="$2" tmp
  tmp="$(mktemp)"
  jq --arg k "${key}" --arg v "${value}" '.[$k] = $v' "${ADDRESSES_FILE}" > "${tmp}"
  mv "${tmp}" "${ADDRESSES_FILE}"
  ok "${key} = ${value}"
}

# -- stellar helpers -----------------------------------------------------------

# Deploy a contract, or reuse the existing id from addresses.json.
deploy_contract() {
  local name="$1" wasm="$2"
  local existing
  existing="$(get_address "${name}")"
  if [ -n "${existing}" ]; then
    warn "${name} already deployed at ${existing} -- skipping deploy"
    echo "${existing}"
    return
  fi

  info "Deploying ${name}..."
  if [ "${DRY_RUN}" -eq 1 ]; then
    echo "[dry-run-id-${name}]"
    return
  fi
  local id
  id="$(stellar contract deploy \
    --wasm "${wasm}" \
    --source "${DEPLOYER}" \
    --network "${NETWORK}" \
    2>&1)" || fail "Deploy of ${name} failed: ${id}"
  # stellar contract deploy prints the contract id as the last line
  id="$(echo "${id}" | tail -1 | tr -d '[:space:]')"
  save_address "${name}" "${id}"
  echo "${id}"
}

# Invoke a contract entrypoint via the deployer identity.
invoke() {
  local id="$1"; shift
  run stellar contract invoke \
    --id "${id}" \
    --source "${DEPLOYER}" \
    --network "${NETWORK}" \
    -- "$@"
}

# Returns 0 (true) if the contract already has a value for the given view fn.
is_initialized() {
  local id="$1" fn="$2"
  local result
  result="$(stellar contract invoke \
    --id "${id}" \
    --source "${DEPLOYER}" \
    --network "${NETWORK}" \
    --send=no \
    -- "${fn}" 2>/dev/null)" || return 1
  [ -n "${result}" ] && [ "${result}" != "null" ]
}

# -- main deployment flow ------------------------------------------------------
load_addresses
DEPLOYER_ADDR="$(stellar keys address "${DEPLOYER}" --network "${NETWORK}")"

# === Step 1: Deploy contracts =================================================
info ""
info "=== Step 1: Deploy contracts ==="

MULTISIG_ID="$(deploy_contract multisig       "${WASM_DIR}/zenith_multisig.wasm")"
ORACLE_ID="$(  deploy_contract price_oracle   "${WASM_DIR}/zenith_price_oracle.wasm")"
TIMELOCK_ID="$(deploy_contract timelock       "${WASM_DIR}/zenith_timelock.wasm")"
VAULT_ID="$(   deploy_contract vault          "${WASM_DIR}/zenith_vault.wasm")"
MARKET_ID="$(  deploy_contract options_market "${WASM_DIR}/zenith_options_market.wasm")"

# === Step 2: Initialize contracts =============================================
info ""
info "=== Step 2: Initialize contracts ==="

# Multisig: single signer (deployer) so the deployer can approve actions
# immediately. Add more signers via propose_signer_change after deployment;
# see docs/governance-handover.md for transferring to proper M-of-N.
if is_initialized "${MULTISIG_ID}" get_threshold; then
  warn "multisig already initialized -- skipping"
else
  info "Initializing multisig (threshold=${MULTISIG_THRESHOLD}, ttl=${MULTISIG_TTL})..."
  invoke "${MULTISIG_ID}" initialize \
    --signers "[\"${DEPLOYER_ADDR}\"]" \
    --threshold "${MULTISIG_THRESHOLD}" \
    --approval_ttl "${MULTISIG_TTL}"
  ok "multisig initialized"
fi

# price_oracle: deployer as admin
if is_initialized "${ORACLE_ID}" get_admin; then
  warn "price_oracle already initialized -- skipping"
else
  info "Initializing price_oracle..."
  invoke "${ORACLE_ID}" initialize --admin "${DEPLOYER_ADDR}"
  ok "price_oracle initialized"
fi

# timelock: deployer as sole proposer, executor, and canceller.
# Transfer proposer role to the multisig/governor later.
if is_initialized "${TIMELOCK_ID}" get_min_delay; then
  warn "timelock already initialized -- skipping"
else
  info "Initializing timelock (min_delay=${TIMELOCK_DELAY}s)..."
  invoke "${TIMELOCK_ID}" initialize \
    --min_delay "${TIMELOCK_DELAY}" \
    --proposers "[\"${DEPLOYER_ADDR}\"]" \
    --executors "[\"${DEPLOYER_ADDR}\"]" \
    --cancellers "[\"${DEPLOYER_ADDR}\"]"
  ok "timelock initialized"
fi

# vault: deployer as admin, COLLATERAL_TOKEN as default token.
# max_pause_duration = 7 days = 604800 seconds.
if is_initialized "${VAULT_ID}" get_admin; then
  warn "vault already initialized -- skipping"
else
  info "Initializing vault..."
  invoke "${VAULT_ID}" initialize \
    --admin "${DEPLOYER_ADDR}" \
    --token "${COLLATERAL_TOKEN}" \
    --max_pause_duration 604800
  ok "vault initialized"
fi

# options_market: deployer as admin, price_oracle as oracle.
if is_initialized "${MARKET_ID}" get_admin; then
  warn "options_market already initialized -- skipping"
else
  info "Initializing options_market..."
  invoke "${MARKET_ID}" initialize \
    --admin "${DEPLOYER_ADDR}" \
    --oracle "${ORACLE_ID}" \
    --collateral_token "${COLLATERAL_TOKEN}" \
    --fee_recipient "${FEE_RECIPIENT}"
  ok "options_market initialized"
fi

# === Step 3: Wire cross-contract references ====================================
info ""
info "=== Step 3: Wire cross-contract references ==="

# price_oracle: add FEEDER_1 as a price reporter
info "Adding FEEDER_1 to price_oracle..."
FEEDER_1_IS="$(stellar contract invoke \
  --id "${ORACLE_ID}" \
  --source "${DEPLOYER}" \
  --network "${NETWORK}" \
  --send=no \
  -- is_feeder --address "${FEEDER_1}" 2>/dev/null || echo false)"
if [ "${FEEDER_1_IS}" = "true" ]; then
  warn "FEEDER_1 (${FEEDER_1}) is already a feeder -- skipping"
else
  invoke "${ORACLE_ID}" add_feeder --feeder "${FEEDER_1}"
  ok "Added feeder: ${FEEDER_1}"
fi

# price_oracle: add optional FEEDER_2
if [ -n "${FEEDER_2:-}" ]; then
  info "Adding FEEDER_2 to price_oracle..."
  FEEDER_2_IS="$(stellar contract invoke \
    --id "${ORACLE_ID}" \
    --source "${DEPLOYER}" \
    --network "${NETWORK}" \
    --send=no \
    -- is_feeder --address "${FEEDER_2}" 2>/dev/null || echo false)"
  if [ "${FEEDER_2_IS}" = "true" ]; then
    warn "FEEDER_2 (${FEEDER_2}) is already a feeder -- skipping"
  else
    invoke "${ORACLE_ID}" add_feeder --feeder "${FEEDER_2}"
    ok "Added feeder: ${FEEDER_2}"
  fi
fi

# vault: register options_market as an authorized integrator
info "Registering options_market as vault integrator..."
INTEGRATOR_SET="$(stellar contract invoke \
  --id "${VAULT_ID}" \
  --source "${DEPLOYER}" \
  --network "${NETWORK}" \
  --send=no \
  -- is_integrator --addr "${MARKET_ID}" 2>/dev/null || echo false)"
if [ "${INTEGRATOR_SET}" = "true" ]; then
  warn "options_market is already a vault integrator -- skipping"
else
  invoke "${VAULT_ID}" set_integrator \
    --integrator "${MARKET_ID}" \
    --allowed true
  ok "vault integrator set: options_market"
fi

# vault: allowlist the collateral token
info "Allowlisting collateral token on vault..."
TOKEN_ALLOWED="$(stellar contract invoke \
  --id "${VAULT_ID}" \
  --source "${DEPLOYER}" \
  --network "${NETWORK}" \
  --send=no \
  -- is_token_allowed --token "${COLLATERAL_TOKEN}" 2>/dev/null || echo false)"
if [ "${TOKEN_ALLOWED}" = "true" ]; then
  warn "Collateral token already allowlisted on vault -- skipping"
else
  invoke "${VAULT_ID}" set_token_allowed \
    --token "${COLLATERAL_TOKEN}" \
    --allowed true
  ok "vault token allowed: ${COLLATERAL_TOKEN}"
fi

# price_oracle: pin the multisig so *_via_multisig entrypoints are usable
info "Pinning multisig on price_oracle..."
EXISTING_MS="$(stellar contract invoke \
  --id "${ORACLE_ID}" \
  --source "${DEPLOYER}" \
  --network "${NETWORK}" \
  --send=no \
  -- get_multisig 2>/dev/null || echo null)"
if [ "${EXISTING_MS}" != "null" ] && [ -n "${EXISTING_MS}" ]; then
  warn "price_oracle multisig already pinned (${EXISTING_MS}) -- skipping"
else
  invoke "${ORACLE_ID}" set_multisig --multisig "${MULTISIG_ID}"
  ok "price_oracle multisig pinned: ${MULTISIG_ID}"
fi

# === Step 4: Write addresses.json =============================================
info ""
info "=== Step 4: Write addresses.json ==="

save_address "network"           "${NETWORK}"
save_address "deployer_address"  "${DEPLOYER_ADDR}"
save_address "feeder_1"          "${FEEDER_1}"
if [ -n "${FEEDER_2:-}" ]; then
  save_address "feeder_2" "${FEEDER_2}"
fi
save_address "collateral_token"  "${COLLATERAL_TOKEN}"
save_address "fee_recipient"     "${FEE_RECIPIENT}"

echo ""
info "Deployment complete."
echo ""
echo "  addresses.json: ${ADDRESSES_FILE}"
echo ""
jq '.' "${ADDRESSES_FILE}"
echo ""
echo "Next steps:"
echo "  Run the smoke test:  scripts/deploy/smoke.sh"
echo "  Governance handover: docs/governance-handover.md"
