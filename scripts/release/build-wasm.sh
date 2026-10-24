#!/usr/bin/env bash
#
# build-wasm.sh - Build every contract crate to wasm and emit a checksum manifest.
#
# This script is the single code path used both locally and by the release
# workflow (.github/workflows/release.yml). Running it locally should produce
# the same artifacts (and, given a pinned toolchain, the same checksums) as CI.
#
# Usage:
#   scripts/release/build-wasm.sh [--out-dir DIR] [--no-build]
#
# Options:
#   --out-dir DIR   Directory to place artifacts in (default: target/release-wasm)
#   --no-build      Skip building; only (re)generate the checksum manifest from
#                   existing artifacts in the output directory.
#   -h, --help      Show this help.
#
# Outputs:
#   <out-dir>/<crate>.wasm   One wasm artifact per contract crate.
#   <out-dir>/SHA256SUMS     Deterministically ordered sha256 manifest.
#
# Exit codes:
#   0  success
#   1  build failure, missing artifact, or empty artifact set
#   2  usage error
#
set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

# Pinned toolchain. Keep in sync with rust-toolchain.toml and the release
# workflow. Overridable for local experimentation, but CI must not override it.
RUST_TOOLCHAIN="${RUST_TOOLCHAIN:-1.79.0}"
WASM_TARGET="wasm32-unknown-unknown"
PROFILE="release"

# Crates that live in the workspace but are NOT contract crates (rlibs, shared
# libraries, tooling). These are excluded from the artifact set. The list is
# intentionally explicit so that adding a new non-contract crate is a conscious
# decision rather than a silent omission.
NON_CONTRACT_CRATES=(
  "common"
  "xtask"
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log() { printf '%s\n' "$*" >&2; }
die() { log "error: $*"; exit 1; }

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

# Resolve the repository root regardless of where the script is invoked from.
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/../.." && pwd)"

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

out_dir="${repo_root}/target/release-wasm"
do_build=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir)
      [[ $# -ge 2 ]] || die "--out-dir requires an argument"
      out_dir="$2"
      shift 2
      ;;
    --no-build)
      do_build=0
      shift
      ;;
    -h|--help)
      usage 0
      ;;
    *)
      log "unknown argument: $1"
      usage 2
      ;;
  esac
done

# Normalise out_dir to an absolute path.
mkdir -p -- "$out_dir"
out_dir="$(cd -- "$out_dir" && pwd)"

cd -- "$repo_root"

# ---------------------------------------------------------------------------
# Discover contract crates from the workspace
# ---------------------------------------------------------------------------

# List workspace member package names. `cargo metadata` is authoritative and
# avoids hand-maintaining a parallel list in this script.
discover_workspace_members() {
  command -v cargo >/dev/null 2>&1 || die "cargo not found in PATH"
  cargo metadata --no-deps --format-version 1 \
    | tr ',' '\n' \
    | sed -n 's/.*"name":"\([^"]*\)".*/\1/p'
}

is_non_contract_crate() {
  local name="$1"
  local excluded
  for excluded in "${NON_CONTRACT_CRATES[@]}"; do
    if [[ "$name" == "$excluded" ]]; then
      return 0
    fi
  done
  return 1
}

# A crate is treated as a contract crate if its source tree contains a
# `#[contract]` attribute. This is a robust, rename-proof signal: a crate that
# stops being a contract is dropped, and a crate that becomes one is picked up.
crate_has_contract_attr() {
  local name="$1"
  local crate_dir
  crate_dir="$(cargo metadata --no-deps --format-version 1 \
    | tr '}' '\n' \
    | grep -F "\"name\":\"${name}\"" \
    | sed -n 's/.*"manifest_path":"\([^"]*\)".*/\1/p' \
    | head -n1)"
  [[ -n "$crate_dir" ]] || return 1
  crate_dir="$(dirname -- "$crate_dir")"
  [[ -d "$crate_dir/src" ]] || return 1
  grep -rqE '#\[[[:space:]]*contract[[:space:]]*\]' "$crate_dir/src"
}

discover_contract_crates() {
  local name
  local found=0
  while IFS= read -r name; do
    [[ -n "$name" ]] || continue
    if is_non_contract_crate "$name"; then
      log "skip (non-contract): $name"
      continue
    fi
    if crate_has_contract_attr "$name"; then
      printf '%s\n' "$name"
      found=1
    else
      log "skip (no #[contract]): $name"
    fi
  done < <(discover_workspace_members | sort -u)

  if [[ "$found" -eq 0 ]]; then
    return 1
  fi
  return 0
}

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

build_crate() {
  local name="$1"
  log "building ${name} (${PROFILE}, ${WASM_TARGET})"
  cargo "+${RUST_TOOLCHAIN}" build \
    --package "$name" \
    --target "$WASM_TARGET" \
    --profile "$PROFILE" \
    --locked
}

# Locate the produced wasm for a crate. Cargo names the artifact after the
# library target (hyphens become underscores).
locate_wasm() {
  local name="$1"
  local lib_name="${name//-/_}"
  local candidate="${repo_root}/target/${WASM_TARGET}/${PROFILE}/${lib_name}.wasm"
  if [[ -f "$candidate" ]]; then
    printf '%s\n' "$candidate"
    return 0
  fi
  return 1
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

mapfile -t contract_crates < <(discover_contract_crates) || true

if [[ "${#contract_crates[@]}" -eq 0 ]]; then
  die "no contract crates discovered in the workspace; refusing to produce an empty release"
fi

log "discovered ${#contract_crates[@]} contract crate(s): ${contract_crates[*]}"

# Deterministic ordering for both build and manifest.
IFS=$'\n' contract_crates=($(printf '%s\n' "${contract_crates[@]}" | LC_ALL=C sort))
unset IFS

# Clean previous artifacts so a stale wasm cannot masquerade as a fresh build.
rm -f -- "${out_dir}"/*.wasm "${out_dir}/SHA256SUMS"

missing=()

if [[ "$do_build" -eq 1 ]]; then
  for crate in "${contract_crates[@]}"; do
    build_crate "$crate"
  done
fi

for crate in "${contract_crates[@]}"; do
  if wasm_path="$(locate_wasm "$crate")"; then
    cp -f -- "$wasm_path" "${out_dir}/${crate}.wasm"
    log "artifact: ${crate}.wasm"
  else
    missing+=("$crate")
  fi
done

if [[ "${#missing[@]}" -gt 0 ]]; then
  die "expected wasm artifact(s) not produced for: ${missing[*]}"
fi

# ---------------------------------------------------------------------------
# Checksum manifest
# ---------------------------------------------------------------------------

# Generate SHA256SUMS with a stable, locale-independent ordering. We compute
# checksums from the copied artifacts in out_dir so the manifest always matches
# exactly what will be uploaded.
(
  cd -- "$out_dir"
  shopt -s nullglob
  wasm_files=( *.wasm )
  if [[ "${#wasm_files[@]}" -eq 0 ]]; then
    die "no wasm artifacts present in ${out_dir}; refusing to write empty manifest"
  fi
  # Sort filenames bytewise for determinism across platforms.
  IFS=$'\n' sorted=( $(printf '%s\n' "${wasm_files[@]}" | LC_ALL=C sort) )
  unset IFS
  for f in "${sorted[@]}"; do
    sha256sum -- "$f"
  done > SHA256SUMS
)

log "wrote ${out_dir}/SHA256SUMS:"
cat -- "${out_dir}/SHA256SUMS" >&2

log "done: ${#contract_crates[@]} artifact(s) in ${out_dir}"