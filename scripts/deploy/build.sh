#!/usr/bin/env bash
# scripts/deploy/build.sh -- build production wasm for every contract in the
# Zenith stack, in dependency order.
#
# Dependency order (each contract must exist in target/ before the next can
# compile its contractimport! macros):
#   1. zenith-multisig     (no wasm-deps)
#   2. zenith-price-oracle (no wasm-deps)
#   3. zenith-timelock     (no wasm-deps)
#   4. zenith-vault        (no wasm-deps)
#   5. zenith-options-market (imports price_oracle, vault)
#
# Usage:
#   scripts/deploy/build.sh                 # build all five contracts
#   scripts/deploy/build.sh --skip-check    # skip wasm-check step
#
# Outputs:
#   target/wasm32-unknown-unknown/release/zenith_multisig.wasm
#   target/wasm32-unknown-unknown/release/zenith_price_oracle.wasm
#   target/wasm32-unknown-unknown/release/zenith_timelock.wasm
#   target/wasm32-unknown-unknown/release/zenith_vault.wasm
#   target/wasm32-unknown-unknown/release/zenith_options_market.wasm
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WASM_DIR="$REPO_ROOT/target/wasm32-unknown-unknown/release"
SKIP_CHECK=0

# -- argument parsing ---------------------------------------------------------
for arg in "$@"; do
  case "$arg" in
    --skip-check) SKIP_CHECK=1 ;;
    --help|-h)
      grep '^#' "$0" | head -20 | sed 's/^# \{0,2\}//'
      exit 0
      ;;
    *) echo "error: unknown argument '$arg'" >&2; exit 1 ;;
  esac
done

# -- prerequisites -------------------------------------------------------------
need() {
  if ! command -v "$1" &>/dev/null; then
    echo "error: '$1' not found -- $2" >&2
    exit 1
  fi
}
need cargo    "install Rust: https://rustup.rs"
need rustup   "install Rust: https://rustup.rs"

if ! rustup target list --installed | grep -q "wasm32-unknown-unknown"; then
  echo "Adding wasm32-unknown-unknown target..."
  rustup target add wasm32-unknown-unknown
fi

# -- helpers -------------------------------------------------------------------
build_contract() {
  local pkg="$1"
  echo ""
  echo "----------------------------------------------"
  echo "Building $pkg"
  echo "----------------------------------------------"
  cargo build -p "$pkg" \
    --target wasm32-unknown-unknown \
    --release \
    --manifest-path "$REPO_ROOT/Cargo.toml"
}

check_wasm() {
  local wasm="$1" name
  name="$(basename "$wasm")"
  if [ ! -f "$wasm" ]; then
    echo "error: expected $wasm -- build may have failed" >&2
    exit 1
  fi
  echo "  [ok] $name  ($(du -sh "$wasm" | cut -f1))"
}

# -- build ---------------------------------------------------------------------
cd "$REPO_ROOT"

build_contract zenith-multisig
build_contract zenith-price-oracle
build_contract zenith-timelock
build_contract zenith-vault
build_contract zenith-options-market

# -- verify outputs ------------------------------------------------------------
echo ""
echo "Verifying wasm artifacts:"
check_wasm "$WASM_DIR/zenith_multisig.wasm"
check_wasm "$WASM_DIR/zenith_price_oracle.wasm"
check_wasm "$WASM_DIR/zenith_timelock.wasm"
check_wasm "$WASM_DIR/zenith_vault.wasm"
check_wasm "$WASM_DIR/zenith_options_market.wasm"

# -- optional cargo xtask check-wasm ------------------------------------------
if [ "$SKIP_CHECK" -eq 0 ] && command -v cargo &>/dev/null; then
  echo ""
  echo "Running cargo xtask check-wasm (no testutils leak check)..."
  cargo xtask check-wasm -p zenith-multisig \
    -p zenith-price-oracle \
    -p zenith-timelock \
    -p zenith-vault \
    -p zenith-options-market \
    || { echo "warning: check-wasm found issues (non-fatal in dev)" >&2; }
fi

echo ""
echo "Build complete. Wasm in $WASM_DIR/"
