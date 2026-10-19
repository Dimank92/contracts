#!/usr/bin/env bash
# Rollback stage 1: the multisig account returns admin to the deployer.
source "$(dirname "$0")/common.sh"
require DEPLOYER MULTISIG_ACCOUNT

invoke "$MARKET_ID" "$MULTISIG_ACCOUNT" transfer_admin --new_admin "$DEPLOYER"
invoke "$ORACLE_ID" "$MULTISIG_ACCOUNT" transfer_admin --new_admin "$DEPLOYER"
show_admins

