#!/usr/bin/env bash
# Public-ABI guard: compares each complete contract's spec at the base
# ref against HEAD with spec-diff, honoring tools/spec-diff/allow.txt,
# and prints each wasm's size delta.
#
# Usage: tools/spec-diff/check.sh <base-ref>
set -euo pipefail

base_ref=${1:?usage: check.sh <base-ref>}
root=$(git rev-parse --show-toplevel)
cd "$root"
allow_file=tools/spec-diff/allow.txt
target=wasm32-unknown-unknown

cargo xtask build
cargo build -q -p spec-diff --release
spec_diff=$root/target/release/spec-diff

base_dir=$(mktemp -d)
trap 'git worktree remove --force "$base_dir" >/dev/null 2>&1 || true' EXIT
git worktree add --detach "$base_dir" "$base_ref" >/dev/null

contracts=$(cargo metadata --no-deps --format-version 1 | python3 -c '
import json, sys
meta = json.load(sys.stdin)
skip = set(meta["metadata"]["zenith"]["incomplete"]) if meta.get("metadata") else set()
for p in meta["packages"]:
    if p["name"] in skip:
        continue
    for t in p["targets"]:
        if "cdylib" in t["kind"]:
            print(p["name"], t["name"].replace("-", "_"))
')

status=0
while read -r crate lib; do
  head_wasm=target/$target/release/$lib.wasm
  if ! (cd "$base_dir" && cargo build -q -p "$crate" --lib --target $target --release \
        --target-dir "$base_dir/target") 2>/dev/null; then
    echo "== $crate: skipped (doesn't build at $base_ref)"
    continue
  fi
  base_wasm=$base_dir/target/$target/release/$lib.wasm
  allows=()
  while read -r c fn; do
    [ "$c" = "$crate" ] && allows+=(--allow "$fn")
  done < <(grep -vE '^\s*(#|$)' "$allow_file")
  echo "== $crate ($(stat -c %s "$base_wasm") -> $(stat -c %s "$head_wasm") bytes)"
  "$spec_diff" "$base_wasm" "$head_wasm" "${allows[@]}" || status=1
done <<< "$contracts"
exit $status
