#!/usr/bin/env bash
# Project-specific lint checks for Zenith contracts.
# Run via: scripts/lint/check.sh
# Exit codes: 0 = all clean, 1 = violations found.
set -euo pipefail

FAILED=0

# ── Check 1: No .unwrap() in entrypoint function bodies ────────────────────
# Entrypoints in Soroban contracts should use panic_with_error! instead of
# .unwrap() so errors are reported with typed contract error codes.
# Heuristic: flag .unwrap() appearing in files that have #[contractimpl].
echo "Checking: no .unwrap() in contract entrypoints..."
for f in $(grep -rl '#\[contractimpl\]' --include='*.rs' .; ); do
  if grep -n '\.unwrap()' "$f" | grep -v '#\[cfg(test)\]' | grep -qv '//'; then
    echo "FAIL: .unwrap() found in entrypoint file: $f"
    grep -n '\.unwrap()' "$f" | grep -v '//'
    FAILED=1
  fi
done

# ── Check 2: require_auth() appears before storage writes in admin fns ─────
# Heuristic: in pub fn bodies inside #[contractimpl], check that
# require_auth appears before the first storage().instance().set or
# storage().persistent().set call.
# NOTE: This is a best-effort grep check; full enforcement requires a
# custom dylint or semgrep rule. This catches the obvious missing-auth cases.
echo "Checking: require_auth present in admin functions..."
for f in $(grep -rl 'fn.*admin\|transfer_admin\|set_fee_rate\|pause\|unpause\|create_series\|cancel_series\|update_premium' --include='*.rs' */src/lib.rs 2>/dev/null || true); do
  if grep -q 'storage.*set\|instance.*set' "$f" && ! grep -q 'require_auth' "$f"; then
    echo "WARN: $f has storage writes but no require_auth — verify manually."
  fi
done

# ── Check 3: No Address parameter named *_contract on trust-sensitive calls
# The _via_multisig pattern accepts a multisig_contract: Address and calls
# is_executable on it. A parameter named *_contract is fine as a cross-
# contract call target (that's intentional), but passing it into functions
# that change local state requires extra care. Flag for manual review.
echo "Checking: *_contract Address params on internal (non-client) functions..."
for f in $(find . -name '*.rs' -path '*/src/*' ! -path '*/test*' ! -path '*/target/*'); do
  if grep -n '_contract.*:.*Address\|Address.*_contract' "$f" | grep -qv 'pub fn\|//'; then
    # Only flag if it's in a non-pub (internal) function signature
    if grep -n 'fn.*_contract.*:.*Address' "$f" | grep -v 'pub fn' | grep -qv '//'; then
      echo "WARN: Internal fn in $f accepts *_contract: Address — verify trust boundary."
      grep -n 'fn.*_contract.*:.*Address' "$f" | grep -v 'pub fn' | grep -v '//'
    fi
  fi
done

if [ "$FAILED" -eq 1 ]; then
  echo ""
  echo "Lint check FAILED. See violations above."
  exit 1
else
  echo ""
  echo "All lint checks passed."
fi
