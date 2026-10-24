#!/usr/bin/env bash
# Stage 0 → 1: deployer hands admin of options_market and price_oracle to
# the multisig account. Rollback: stage1_rollback.sh (multisig → deployer).
source "$(dirname "$0")/common.sh"
require DEPLOYER MULTISIG_ACCOUNT

show_admins
invoke "$MARKET_ID" "$DEPLOYER" transfer_admin --new_admin "$MULTISIG_ACCOUNT"
invoke "$ORACLE_ID" "$DEPLOYER" transfer_admin --new_admin "$MULTISIG_ACCOUNT"
show_admins

