#!/usr/bin/env bash
# Wasm size gate (#106).
#
# For every crate named on the command line (default: all contracts):
#   1. reads the release wasm (build it first with
#      `cargo build --target wasm32-unknown-unknown --release`),
#   2. writes an optimized copy with `stellar contract optimize` if the
#      stellar CLI is installed, else `wasm-opt -Oz` (binaryen),
#   3. prints raw and optimized sizes as a Markdown table,
#   4. fails if an optimized size exceeds its budget in budgets.txt.
#
# Usage:
#   scripts/wasm-size/check.sh [--update] [--in-place] [crate...]
#
#   --update    rewrite budgets.txt as optimized size + 10% (rounded up)
#               for the crates checked; use when a size increase is
#               deliberate, and say why in the PR.
#   --in-place  also replace the release wasm with the optimized one, so
#               later steps (resource snapshots, uploads) use it.
#
# Optimized copies land in target/wasm-opt/ at the repo root. Set
# GITHUB_STEP_SUMMARY to append the table to the CI job summary.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
budgets="$root/scripts/wasm-size/budgets.txt"
out_dir="$root/target/wasm-opt"
mkdir -p "$out_dir"

update=0
in_place=0
crates=()
for arg in "$@"; do
  case "$arg" in
    --update) update=1 ;;
    --in-place) in_place=1 ;;
    *) crates+=("$arg") ;;
  esac
done
if [ ${#crates[@]} -eq 0 ]; then
  # The complete contracts (README "Known gaps" lists the others).
  crates=(governor grants_escrow lp_pool lp_token multisig params staking streams)
fi

optimize() {
  if command -v stellar >/dev/null 2>&1; then
    stellar contract optimize --wasm "$1" --wasm-out "$2" >/dev/null
  elif command -v wasm-opt >/dev/null 2>&1; then
    wasm-opt -Oz --strip-debug --strip-producers \
      --enable-sign-ext --enable-mutable-globals "$1" -o "$2"
  else
    echo "neither stellar nor wasm-opt found" >&2
    exit 2
  fi
}

budget_of() {
  awk -v c="$1" '$1 == c { print $2 }' "$budgets"
}

set_budget() {
  local tmp
  tmp=$(mktemp)
  awk -v c="$1" -v b="$2" '
    $1 == c { print c, b; done = 1; next }
    { print }
    END { if (!done) print c, b }
  ' "$budgets" >"$tmp"
  mv "$tmp" "$budgets"
}

table="| Contract | Raw (bytes) | Optimized (bytes) | Saved | Budget | Headroom |
|---|---:|---:|---:|---:|---:|"
status=0

for c in "${crates[@]}"; do
  pkg=$(awk -F'"' '/^name *=/ { print $2; exit }' "$root/$c/Cargo.toml" 2>/dev/null || true)
  # The workspace's shared target/ (`cargo xtask build`).
  wasm="$root/target/wasm32-unknown-unknown/release/${pkg//-/_}.wasm"
  if [ -z "$pkg" ] || [ ! -f "$wasm" ]; then
    table+=$'\n'"| \`$c\` | not built | | | | |"
    continue
  fi
  opt="$out_dir/$(basename "$wasm")"
  optimize "$wasm" "$opt"
  raw_size=$(wc -c <"$wasm")
  opt_size=$(wc -c <"$opt")
  saved=$(awk -v r="$raw_size" -v o="$opt_size" 'BEGIN { printf "%.1f%%", (r - o) * 100 / r }')

  if [ $update -eq 1 ]; then
    set_budget "$c" $(((opt_size * 110 + 99) / 100))
  fi
  budget=$(budget_of "$c")
  if [ -z "$budget" ]; then
    headroom="no budget"
    status=1
  else
    headroom=$(awk -v b="$budget" -v o="$opt_size" 'BEGIN { printf "%.1f%%", (b - o) * 100 / b }')
    if [ "$opt_size" -gt "$budget" ]; then
      headroom="**OVER by $((opt_size - budget))**"
      status=1
    fi
  fi
  table+=$'\n'"| \`$c\` | $raw_size | $opt_size | $saved | ${budget:-–} | $headroom |"

  if [ $in_place -eq 1 ]; then
    cp "$wasm" "${wasm%.wasm}.raw.wasm"
    cp "$opt" "$wasm"
  fi
done

echo "$table"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Wasm sizes"
    echo
    echo "$table"
  } >>"$GITHUB_STEP_SUMMARY"
fi
if [ $status -ne 0 ]; then
  echo "wasm size budget exceeded or missing (see table). If intended, run" >&2
  echo "  scripts/wasm-size/check.sh --update <crate>" >&2
  echo "and explain the increase in the PR." >&2
fi
exit $status
