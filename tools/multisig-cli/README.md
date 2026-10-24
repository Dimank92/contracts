# multisig-cli

Collects approvals for a `multisig` action and submits it, so signers do not
hand-craft Soroban auth entries. The contract stays the only source of truth:
the CLI asks `is_approved` / `is_executable` and never re-implements the
threshold or expiry rules. Shells out to `stellar-cli`; key custody, HSMs and
UIs are out of scope.

Common flags: `--multisig <C...>`, `--rpc-url`, `--network-passphrase`
(default testnet), `--json` for compact machine-readable output.

```sh
# 1. The exact payload + per-signer unsigned-tx commands (offline signers)
multisig-cli payload --multisig $MS --target $VAULT --function pause_via_multisig \
  --args '{"caller":"G..."}' --signers G1,G2

# 2. Collect: stellar-cli identities approve now, or submit pre-signed XDR files
multisig-cli collect --multisig $MS --action-id $ID --signer alice --signer bob
multisig-cli collect --multisig $MS --action-id $ID --signed bob.xdr --dry-run

# 3. Status: weights, who approved, threshold, approval_ttl, execute_by
multisig-cli status --multisig $MS --action-id $ID [--class 1] --json

# 4. Submit the action once the contract reports it approved
multisig-cli submit --multisig $MS --action-id $ID --target $VAULT \
  --function pause_via_multisig --args '{"caller":"G..."}' --source alice
```

`action_id` is `sha256` of the canonical `{target, function, args}` JSON, so
one id can never be reused for a different payload (`submit` re-derives and
checks it). `submit` re-reads the contract right before sending, so a changed
signer set or an expired approval is refused. Wrong network: pass the matching
`--rpc-url`/`--network-passphrase`; a standalone sandbox works the same way.

Local sandbox: `--rpc-url http://localhost:8000/soroban/rpc
--network-passphrase 'Standalone Network ; February 2017'`.

`cargo test -p multisig-cli` runs the logic against the real contract in the
Soroban test environment: happy path, one signature short, expired approval,
signer-set change mid-flight, dry-run/pre-signed, and golden JSON.
