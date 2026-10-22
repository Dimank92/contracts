#!/usr/bin/env bash
# Reproducible-wasm gate (#260).
#
# Builds every contract twice from a pristine target/ and fails unless the
# sha256 of each .wasm is identical across both builds. Absolute paths
# (checkout, CARGO_HOME, rustup sysroot) are remapped so they can't leak
# into the binary. target/ itself must stay at the repo root because
# options_market contractimport!s its dependencies' wasm from there, so the
# two builds run back to back, each from a clean wasm output directory.
#
# Usage: scripts/reproducible-build/verify.sh [--print] [crate...]
#   --print   only build once and print "<sha256>  <crate>.wasm" lines
#             (what you compare against a published hash)
#   env VERIFY_INJECT_DRIFT=1 appends a build-time nonce, proving the gate
#   fails on non-determinism (see the negative check in CI).
set -euo pipefail

root=$(git rev-parse --show-toplevel)
cd "$root"

print_only=0
[ "${1:-}" = "--print" ] && { print_only=1; shift; }
crates=("$@")
[ ${#crates[@]} -eq 0 ] && crates=(multisig price_oracle vault params options_market streams staking governor grants_escrow timelock)

cargo_home=${CARGO_HOME:-$HOME/.cargo}
sysroot=$(rustc --print sysroot)
flags="-C debuginfo=0 -C strip=symbols -C codegen-units=1"
flags+=" --remap-path-prefix=$root=/build"
flags+=" --remap-path-prefix=$cargo_home=/cargo"
flags+=" --remap-path-prefix=$sysroot=/rustc"
[ "${VERIFY_INJECT_DRIFT:-0}" = "1" ] && flags+=" --cfg drift_$(date +%s%N)"
export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="$flags"
export CARGO_INCREMENTAL=0
export SOURCE_DATE_EPOCH=0

lock_before=$(sha256sum Cargo.lock)

build_once() { # $1 = output dir of hashes
  rm -rf "$root/target/wasm32-unknown-unknown"
  local pkgs=()
  for c in "${crates[@]}"; do pkgs+=("-p" "$c"); done
  cargo xtask build "${pkgs[@]}" >&2
  : > "$1"
  for c in "${crates[@]}"; do
    f="$root/target/wasm32-unknown-unknown/release/zenith_${c}.wasm"
    [ -f "$f" ] || { echo "missing $f" >&2; exit 1; }
    echo "$(sha256sum "$f" | cut -d' ' -f1)  zenith_${c}.wasm" >> "$1"
  done
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
build_once "$tmp/a"
if [ $print_only -eq 1 ]; then cat "$tmp/a"; exit 0; fi
build_once "$tmp/b"

echo "== build 1 =="; cat "$tmp/a"
echo "== build 2 =="; cat "$tmp/b"
if ! diff -u "$tmp/a" "$tmp/b"; then
  echo "FAIL: wasm builds are not reproducible" >&2
  exit 1
fi
if [ "$lock_before" != "$(sha256sum Cargo.lock)" ]; then
  echo "FAIL: build modified Cargo.lock" >&2
  exit 1
fi
echo "OK: all ${#crates[@]} contracts reproduce byte-for-byte"
