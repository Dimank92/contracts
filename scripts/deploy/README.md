# scripts/deploy/ — Zenith testnet deployment scripts

Three shell scripts that take you from compiled wasm to a working Zenith
contract set on Soroban testnet, in order:

```
scripts/deploy/build.sh    # 1. compile wasm (Rust → wasm32)
scripts/deploy/deploy.sh   # 2. deploy + initialize + wire all contracts
scripts/deploy/smoke.sh    # 3. end-to-end smoke test against the live set
```

All three are idempotent: re-running after a partial failure is safe.
Addresses are written to (and re-loaded from) `scripts/deploy/addresses.json`
on each run.

---

## Prerequisites

| Tool | Install |
|------|---------|
| Rust + wasm target | `rustup target add wasm32-unknown-unknown` |
| stellar CLI | [developers.stellar.org/docs/tools/cli](https://developers.stellar.org/docs/tools/cli/stellar-cli) |
| jq | `apt install jq` / `brew install jq` |

Configure the stellar CLI for testnet once:

```sh
stellar network add testnet \
  --rpc-url  https://soroban-testnet.stellar.org \
  --network-passphrase "Test SDF Network ; September 2015"
```

Fund your deployer identity (creates a testnet Stellar account):

```sh
stellar keys generate alice --network testnet
stellar account fund alice --network testnet     # uses the Stellar Friendbot
```

---

## Step 1 — Build wasm

```sh
scripts/deploy/build.sh
```

This runs `cargo build --target wasm32-unknown-unknown --release` for
every contract in dependency order:

1. `zenith-multisig`
2. `zenith-price-oracle`
3. `zenith-timelock`
4. `zenith-vault`
5. `zenith-options-market` (imports price_oracle and vault wasm)

Output wasm lands in `target/wasm32-unknown-unknown/release/`.

Options:

| Flag | Effect |
|------|--------|
| `--skip-check` | Skip the post-build `cargo xtask check-wasm` step |

---

## Step 2 — Deploy and wire

```sh
export DEPLOYER=alice                    # stellar-cli identity name
export COLLATERAL_TOKEN=C...             # USDC or other SAC on testnet
export FEE_RECIPIENT=G...                # protocol fee destination

scripts/deploy/deploy.sh
```

What happens:

1. **Deploy** five contracts (skips any already in `addresses.json`)
2. **Initialize** each one (detects `AlreadyInitialized` and skips)
3. **Wire** cross-contract references:
   - `price_oracle.add_feeder(FEEDER_1)` (and `FEEDER_2` if set)
   - `vault.set_integrator(options_market, true)`
   - `vault.set_token_allowed(COLLATERAL_TOKEN, true)`
   - `price_oracle.set_multisig(multisig)` (pins the multisig)
4. **Write** all addresses to `scripts/deploy/addresses.json`

Optional environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `FEEDER_1` | deployer's own address | First oracle price feeder |
| `FEEDER_2` | _(unset)_ | Second oracle feeder (adds to aggregate) |
| `NETWORK` | `testnet` | stellar-cli network name |
| `ADDRESSES_FILE` | `scripts/deploy/addresses.json` | Output file |
| `TIMELOCK_DELAY` | `3600` | Timelock minimum delay in seconds |
| `MULTISIG_THRESHOLD` | `1` | M-of-N threshold (1 = single-signer) |
| `MULTISIG_TTL` | `0` | Approval TTL seconds (0 = never expires) |
| `WASM_DIR` | `target/wasm32-unknown-unknown/release` | Wasm location |

Dry run (prints commands, does not send transactions):

```sh
scripts/deploy/deploy.sh --dry-run
```

Secrets (deployer keys) are read from the stellar-cli keystore via the
`DEPLOYER` identity name. The raw private key is never passed as an
argument, written to a file, or logged.

---

## Step 3 — Smoke test

```sh
export DEPLOYER=alice
export BUYER=bob                 # holds COLLATERAL_TOKEN
export WRITER=carol              # holds COLLATERAL_TOKEN (for collateral)

scripts/deploy/smoke.sh
```

The smoke test drives the full happy path:

| Step | Action |
|------|--------|
| 1 | `price_oracle.report_price(feeder, XLM, price)` |
| 2 | `price_oracle.get_price(XLM)` — verifies aggregate |
| 3 | `options_market.create_series(XLM, CALL, strike, expiry, premium)` |
| 4 | `options_market.buy_option(buyer, series_id, contracts, max_premium)` |
| 5 | `options_market.write_option(writer, series_id, contracts, collateral)` |
| 6 | _(wait for ledger time > expiry)_ |
| 7 | `options_market.set_settlement_price_from_oracle(series_id, oracle)` |
| 8 | `options_market.exercise(buyer, long_position_id)` |
| 9 | `options_market.reclaim_collateral(writer, short_position_id)` |

Steps 7–9 require the series to be expired on-chain. The script attempts
them immediately and warns rather than failing hard when the series has not
yet expired (timing-dependent on testnet block time). Re-running the smoke
test after the expiry timestamp completes the flow.

Optional overrides:

| Variable | Default | Description |
|----------|---------|-------------|
| `ADDRESSES_FILE` | `scripts/deploy/addresses.json` | From deploy.sh |
| `NETWORK` | `testnet` | stellar-cli network name |
| `STRIKE_PRICE` | `1500000` | Strike price at 7 decimals |
| `SETTLEMENT_PRICE` | `2000000` | Price feeder reports (ITM for CALL) |
| `CONTRACTS` | `10000000` | Contracts at PRICE_PRECISION (= 1.0) |
| `PREMIUM` | `100000` | Per-contract premium at 7 decimals |

---

## addresses.json

`deploy.sh` writes and `smoke.sh` reads `scripts/deploy/addresses.json`.
The file is machine-readable JSON:

```json
{
  "multisig":         "C...",
  "price_oracle":     "C...",
  "timelock":         "C...",
  "vault":            "C...",
  "options_market":   "C...",
  "network":          "testnet",
  "deployer_address": "G...",
  "feeder_1":         "G...",
  "collateral_token": "C...",
  "fee_recipient":    "G..."
}
```

**Never commit `addresses.json` to source control** if it also contains
secrets. The file holds only contract IDs and public addresses — no
private keys — so it is safe to commit for team-shared testnet environments.

---

## Out of scope

- **Mainnet deployment** — requires a proper governance handover, a funded
  multisig quorum, and timelock delays much longer than 1 hour.  
  See `docs/governance-handover.md`.
- **CI gating on a live network** — the smoke test is a manual step;
  unit/integration tests run against the simulated environment in CI.
- **Upgrade / migration** — see `docs/migrations.md`.
- **lp_pool / lp_token / staking** — not part of the core settlement
  stack; add analogous steps when those contracts are ready.

---

## Troubleshooting

| Error | Likely cause | Fix |
|-------|-------------|-----|
| `error: 'stellar' not found` | stellar CLI not installed | See Prerequisites |
| `DEPLOYER must be set` | env var missing | `export DEPLOYER=<identity>` |
| `Deploy of X failed` | account not funded | `stellar account fund $DEPLOYER --network testnet` |
| `AlreadyInitialized` | contract re-initialized | Normal — script skips it |
| `Wasm not found: …` | build hasn't run | `scripts/deploy/build.sh` first |
| `get_price returned null` | feeder not added / stale | Check `FEEDER_1` env var and re-run deploy.sh |
| Settlement step SKIPPED | series not yet expired | Wait for testnet time to advance and re-run smoke.sh |
