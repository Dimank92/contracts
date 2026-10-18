#!/usr/bin/env bash
# Rollback stage 1: the multisig account returns admin to the deployer
# (two-step: the multisig proposes, the deployer accepts).
source "$(dirname "$0")/common.sh"
require DEPLOYER MULTISIG_ACCOUNT

propose_both "$MULTISIG_ACCOUNT" "$DEPLOYER"
accept_both "$DEPLOYER"
show_admins
