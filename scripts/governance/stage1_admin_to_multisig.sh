#!/usr/bin/env bash
# Stage 0 → 1: deployer hands admin of options_market and price_oracle to
# the multisig account (two-step: the deployer proposes, the multisig
# accepts). Rollback: stage1_rollback.sh (multisig → deployer).
source "$(dirname "$0")/common.sh"
require DEPLOYER MULTISIG_ACCOUNT

show_admins
show_trusted_multisig
propose_both "$DEPLOYER" "$MULTISIG_ACCOUNT"
# Built only (unsigned XDR) for the M-of-N signers; until they submit it
# the deployer is still admin and can re-propose.
accept_both "$MULTISIG_ACCOUNT"
show_admins
