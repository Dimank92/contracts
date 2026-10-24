#!/usr/bin/env bash
# Thin wrapper: scripts/multisig-cli.sh payload|status|collect|submit ...
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run -q -p multisig-cli -- "$@"
