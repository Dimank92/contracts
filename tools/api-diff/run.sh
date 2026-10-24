#!/usr/bin/env bash
# Usage: tools/api-diff/run.sh check|update
#   check   build the contract wasm and diff each against its committed
#           baseline in tools/api-diff/baselines/ (CI runs this).
#   update  regenerate the baselines; review the JSON diff in your PR.
# Contracts that do not build to wasm yet are skipped.
set -euo pipefail
cd "$(dirname "$0")/../.."
mode=${1:?usage: run.sh check|update}
CONTRACTS=${CONTRACTS:-"multisig governor grants-escrow params staking streams"}
wasm=()
for c in $CONTRACTS; do
  cargo build -p "zenith-$c" --lib --target wasm32-unknown-unknown --release
  wasm+=("target/wasm32-unknown-unknown/release/zenith_${c//-/_}.wasm")
done
cargo run -q -p api-diff -- "$mode" --baselines tools/api-diff/baselines "${wasm[@]}"
