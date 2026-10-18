# Integration tests

`integration_tests/` is a test-only crate that deploys the real Zenith
contracts into one Soroban `Env` and drives them only through their
public entrypoints. It runs offline (no network, no deployed endpoints)
under plain `cargo test`.

## Running

```sh
cargo xtask test -p integration_tests      # builds the wasm options_market imports, then tests
cargo test -p zenith-integration-tests --test lifecycle -- --nocapture   # one suite, once the wasm exists
```

CI runs it with every other crate's tests (`cargo xtask test`). A renamed
or removed entrypoint breaks compilation of these suites, so it fails
loudly rather than silently skipping a step.

## Suites

| File | What it proves |
|---|---|
| `tests/lifecycle.rs` | The full option lifecycle across price_oracle, vault and options_market: write with collateral escrowed in the vault under the series' tag, buy, feeders report (including an outlier), `aggregate`, settle via `set_settlement_price_from_oracle`, buyer exercises, writer reclaims, escrow ends at zero. Failure paths at the seams: settling before expiry, settling without an oracle quorum, a payout against a vault tag funded by a different series, and substituting a rogue oracle or vault. Also checks the heaviest cross-contract calls fit the default resource budget. |
| `tests/timelock_admin.rs` | The timelock as live admin: two-step hand-off through a timelock operation, rejection before the delay, open execution after it, cancellers, predecessor ordering, recovery from a wrong or misconfigured hand-off, the emergency pause path via the trusted multisig, and the timestamp clock. |
| `tests/handover.rs` | The staged governance handover (deployer → multisig account → timelock → governor → lock-in) and the rollback at every stage. |

## Fixtures

`tests/fixtures/mod.rs` is the one place deployment lives. `deploy()`
returns a `Protocol` with every contract initialized and wired:

- USDC as a Stellar Asset Contract;
- a 2-of-3 `multisig`, set as the trusted multisig of market, oracle and vault;
- `price_oracle` with three feeders and `min_reports = 2`;
- `vault`, handed (two-step) to the market and pinned as its vault, so
  every series listed in a test is vault-backed;
- `options_market` reading `price_oracle` for permissionless settlement.

Helpers cover user actions (`user`, `list_series`, `report`, `warp_to`,
`approve`), timelock operations (`timelock`, `schedule`, `call`,
`accept_calls`), explicit single-signer auth (`sign`) and event
inspection (`event_names`, `last_event_data`).

Rules for new scenarios:

- Call `deploy()` in each test; never share state between tests, so they
  pass in any order.
- Auth is mocked with plain `mock_all_auths()`, not the
  `_allowing_non_root_auth` variant: a contract calling another on its
  own behalf must authorize the nested call as it would on-chain. To
  check "X can and Y can't", call `env.set_auths(&[])` and authorize one
  call with `sign`.
- Control time with `warp`/`warp_to` (ledger timestamp). Don't jump the
  ledger sequence by months: persistent entries archive and the host
  aborts the test.
- Don't call `budget().reset_unlimited()`; scenarios must fit the default
  budget.
- Assert on emitted events as well as return values when the event is
  part of the contract's indexer-facing interface.
