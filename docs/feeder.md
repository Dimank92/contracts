# Feeder daemon (`tools/feeder`)

Reference daemon that submits operator-supplied prices to `price_oracle`.

## Setup
1. Copy the config shape from the tests (`rpc_url`, `network_passphrase`, `oracle_contract_id`,
   `symbols[{name,decimals,price}]`, `submit_interval_secs`, `staleness_window_secs`,
   `staleness_warn_secs`). No network defaults are shipped.
2. The feeder must be registered and staked (see `docs/staking-economics.md`).
3. `cargo run -p zenith-feeder -- --config feeder.json --dry-run` prints payloads as JSON lines.

Submission shells out to the `stellar` CLI (`submit_price(symbol, price, decimals)`; rename in
`StellarCliSubmitter::function` if the oracle entrypoint changes). Prices are scaled to each symbol's
decimals; extra fractional digits are an error.

## Key handling
Secret comes from the env var named by `secret_env` (default `FEEDER_SECRET_KEY`) or a `keyfile`
with mode `0600`. It is passed to the child process by environment only and redacted from errors.

## Running
systemd: `ExecStart=/usr/local/bin/zenith-feeder --config /etc/zenith/feeder.json`,
`Environment=` via `EnvironmentFile=/etc/zenith/feeder.env` (0600), `Restart=on-failure`.
Docker: mount the config read-only, pass the secret with `--env-file`.

## Failure modes
- Transient RPC errors: bounded retries, `backoff_base_ms * 2^n` (cap 60s).
- Contract rejection (outlier/quorum): not retried; process exits 1.
- Last accepted report older than `staleness_warn_secs`: `warn` log; older than the window: exit 1.
- A symbol that cannot be priced is skipped; others still report.
- Reports carry `interval_id = now / submit_interval_secs`, so a restart within one interval is idempotent.
