# Zenith Contracts

Soroban (Stellar smart contract) crates for the Zenith options protocol.

## Crates

- [`options_market/`](options_market) — European-style put/call options on
  XLM, BTC, ETH, and SOL. Premium is set by the admin (computed off-chain via
  Black-Scholes); writers lock collateral, buyers pay premium, settlement
  happens at expiry against an oracle-reported price.
- [`price_oracle/`](price_oracle) — the on-chain logic behind that
  oracle-reported price: admin-authorized feeders report per-symbol
  prices, and the aggregate is freshness-filtered, outlier-filtered (MAD
  band), the median of what survives, and refused unless an
  admin-settable quorum of fresh reports (`min_reports`) survives — so a
  single feeder can't determine the price because every other feeder
  went stale or was removed. `aggregate` records quorum-backed rounds.
  `options_market::set_settlement_price_from_oracle` reads it to settle
  a series permissionlessly. See "[Aggregation](#aggregation)".
- [`vault/`](vault) — a per-tag escrow ledger for a single token,
  motivated by options_market originally holding every writer's
  collateral and every buyer's premium in one undifferentiated balance.
  `deposit`/`withdraw` are scoped to a `u64` tag (options_market uses its
  `series_id`), so a withdrawal can never draw down more than was
  deposited under that tag. Series listed while a vault is pinned escrow
  their writers' collateral in it from the first write.
- [`timelock/`](timelock) — a delayed-execution admin (proposer /
  executor / canceller roles, guardian until lock-in, predecessor
  dependencies) that holds the admin role on the other contracts so users
  always get an exit window. See
  [docs/runbooks/timelock-handoff.md](docs/runbooks/timelock-handoff.md).
- [`lp_pool/`](lp_pool) — a liquidity-provider pool that writes options
  on `options_market` on behalf of its depositors, with epoch-queued
  deposits and withdrawals. See [docs/lp_pool.md](docs/lp_pool.md).
- [`lp_token/`](lp_token) — a SEP-41 share token that only the pinned
  `lp_pool` can mint. See [docs/lp_token.md](docs/lp_token.md).
- [`multisig/`](multisig) — M-of-N approval tracking for opaque,
  caller-defined actions, motivated by every other contract here having
  a single `admin: Address` as its sole point of control. A fixed
  signer set and threshold, `is_approved(action_id)` that flips true
  once enough signers have approved — `action_id` is never interpreted
  by this contract, only counted. Approvals carry an immutable
  `approval_ttl` (set at `initialize`, zero = never expires) so a vote
  cast for a long-abandoned action can't silently still be sitting at
  threshold if that `action_id` is ever reused later. `options_market`,
  `price_oracle`, and `vault` each cross-call their trusted Multisig
  deployment's `is_approved(action_id)` as a permissionless alternative
  to their own admin-gated functions. It can also act as a Soroban **custom account**
  (`__check_auth`) — set its address as a plain `admin` anywhere and
  authorize calls with M-of-N ed25519 signatures.
- [`common/`](common) — `zenith-common`, a plain rlib (no `#[contract]`,
  no exported spec entries) shared by options_market, price_oracle and
  vault: `require_admin_or_multisig(env, admin_key, &Auth, class, err)`
  with `enum Auth { Admin, Multisig(multisig, action_id: BytesN<32>) }`
  (trusted multisig only, one call per approval), plus pause and two-step
  admin-transfer helpers. Every admin function is a single
  `x_inner(env, Auth, ...)` called by both its admin-gated entrypoint
  and its `_via_multisig` twin, so no validation is duplicated between
  twins — see "[Admin functions and `Auth`](#admin-functions-and-auth)".
  Also hosts the storage-migration framework
  (`zenith_common::migrations`) and trusted-multisig upgrades
  (`zenith_common::upgrade`) that every contract uses. See
  [Upgrades and storage migrations](#upgrades-and-storage-migrations).
- [`params/`](params) — a timelock-controlled registry of protocol
  parameters, each with hard `[min, max]` bounds. `options_market`
  reads its fee rate and settlement window from it (cached locally,
  refreshed only when the registry version moves). See
  "`params` reference" below for the parameter catalog.
- [`grants_escrow/`](grants_escrow) — milestone-based escrow for
  community grants and bounties: funded upfront, released per milestone
  on reviewer quorum, reclaimable by the funder after a missed deadline
  plus grace period. Operations guide:
  [`docs/grants_escrow.md`](docs/grants_escrow.md).
- [`test_adversaries/`](test_adversaries) — **dev-dependency only, never
  deployed**. A reusable library of adversarial Soroban contracts for
  negative-path testing at every cross-contract trust boundary (issue #115).
  20 adversarial contracts across four modules:
  - `tokens`: `LyingBalanceToken`, `FeeOnTransferToken`, `RevertOnTransferToken`,
    `ReentrantToken`, `ZeroBalanceToken`, `OverflowToken`
  - `oracle`: `AlwaysNoneOracle`, `ExtremeHighOracle`, `ExtremeLowOracle`,
    `RevertingOracle`, `StaleOracle`, `NegativePriceOracle`
  - `vault`: `NoOpVault`, `WrongBalanceVault`, `RevertOnWithdrawVault`, `DrainVault`
  - `multisig`: `AlwaysApproveMultisig`, `NeverApproveMultisig`,
    `SingleSignerMultisig`, `ExpiredApprovalMultisig`

  Each adversary is a standalone `#[contract]` that mirrors the interface of
  the real contract it spoofs. `options_market`, `price_oracle`, and `vault`
  each add a `src/test_adversaries.rs` with one negative test per
  cross-contract call site. Tests that expose open vulnerabilities are
  `#[ignore]`'d with the issue title and un-ignored by the fix PRs.
  See [`test_adversaries/CATALOG.md`](test_adversaries/CATALOG.md) for the
  full adversary catalog with per-adversary documentation.

## Building and testing

The repo is one Cargo workspace (root `Cargo.toml`: shared
`[profile.release]` and `soroban-sdk` version). Build order is handled by
`cargo xtask` — you need Rust stable plus the wasm target:

```sh
rustup target add wasm32-unknown-unknown
cargo xtask build          # every contract's wasm, dependencies first
cargo xtask test           # builds the wasm tests need, then cargo test
cargo xtask ci             # fmt --check, clippy, wasm checks, tests — same as CI
```

Add `-p <crate>` (e.g. `-p options_market`) to scope any command. Release
wasm lands in `target/wasm32-unknown-unknown/release/`. `cargo xtask help`
lists the rest (`clippy`, `fmt`, `graph`, `check-wasm`, `check-imports`).

Why an xtask: options_market calls price_oracle, vault and params through
`contractimport!` against their *compiled wasm* (a source dependency
would link their `pause`/`transfer_admin`/... into options_market's wasm
and collide), so that wasm must exist before options_market compiles at
all. A crate that imports wasm lists it in `[package.metadata.zenith]
wasm-deps`; xtask builds those first, and `check-imports` keeps the list
in sync with the `contractimport!` paths. Each contract's wasm is built on
its own so dev-dependency `testutils` never reaches a release build —
`check-wasm` verifies that on the artifacts.

### Integration tests

`integration_tests/` deploys the real contracts into one Soroban `Env`
and drives them only through their public entrypoints, offline (no
network). Shared deployment fixtures live in
`integration_tests/tests/fixtures/mod.rs`.

| Suite | Covers |
|---|---|
| `tests/lifecycle.rs` | The full option lifecycle: write with collateral escrowed in the vault, buy, feeders report, settle through `price_oracle`, both sides claim — plus settling before expiry, settling without quorum, a payout against a tag funded by a different series, rogue oracle/vault substitution, and a default-budget check. |
| `tests/timelock_admin.rs` | The timelock as live admin: two-step hand-off, rejection before the delay, cancellation, predecessors, recovery from a wrong or misconfigured hand-off, and the emergency pause path. |
| `tests/handover.rs` | The staged governance handover (deployer → multisig → timelock → governor → lock-in) and every rollback. |

```sh
cargo xtask test -p integration_tests   # builds the wasm options_market imports, then runs all three
cargo test -p zenith-integration-tests --test lifecycle   # one suite, once the wasm exists
```

They run in CI with every other crate's tests (`cargo xtask test`), and
fail to compile if an entrypoint they use is renamed or removed. See
[docs/integration-tests.md](docs/integration-tests.md) for how to add a
scenario.

### Wasm size budgets

The CI `budgets` job builds every contract, optimizes it
(`stellar contract optimize`, or `wasm-opt -Oz` when the stellar CLI
isn't installed), prints raw and optimized sizes to the job summary,
uploads the optimized wasm as the `optimized-wasm` artifact, and fails
if any optimized size is over its budget in
`scripts/wasm-size/budgets.txt` (the size when set + 10%). Locally:

```sh
scripts/wasm-size/check.sh                           # after building the wasm
scripts/wasm-size/check.sh --update options_market   # deliberate growth: explain it in the PR
```

### Resource budget snapshots

The measuring and comparing harness lives in `tools/resource-snapshot`
(CPU instructions, memory bytes, read/write ledger entries and bytes per
entrypoint, compared against `snapshots/resources/*.json` with a 5%
tolerance; run against the optimized wasm with `--features
resource-wasm`, since native execution undercounts).
`options_market/src/test_resources.rs` was written for a later
options_market layout (pruning, `migrate_counters`) that isn't part of
this build, so it is not compiled and no snapshots are recorded yet.
`integration_tests/tests/lifecycle.rs` checks that the heaviest
cross-contract calls fit the default budget in the meantime.

Every event any of these four contracts publishes has a test that
decodes its actual payload via `TryFromVal` (topics and data), not
just a test that confirms an event fired — the intent being that
anything an off-chain indexer would need to parse out of an event is
pinned down by a test, so a change to a tuple's field order or type
shows up as a test failure rather than as a silently broken indexer.

### Property tests

The math modules have [proptest](https://docs.rs/proptest) suites
(`mod proptests` next to each function) that check algebraic properties
over whole input domains rather than hand-picked examples. Every
property runs 10,000 cases, pinned in code, so plain `cargo test` in
CI runs the full count. Realistic domains are prices from 1e3 to 1e15
and contract sizes from 1 to 1e12. Overflow properties sample the whole
`i128` domain instead.

When proptest finds a failure it shrinks it and writes the seed to
`<crate>/proptest-regressions/`. Commit that file with the fix so the
case is replayed on every future run.

| Function | Properties |
|---|---|
| `options_market::math::calc_fee` | `0 <= fee <= amount` for `bps <= 10_000`; equals `floor(amount × bps / 10_000)`; monotonic in amount and in rate; zero when either argument is zero; on the whole `i128` domain, returns the checked result or fails exactly where `amount × bps` overflows |
| `options_market::math::calc_payout` | non-negative; equals `floor(contracts × intrinsic / PRICE_PRECISION)`; zero at or out of the money; calls non-decreasing and puts non-increasing in settlement price; monotonic in contracts; additive in contracts within 1 unit; scaling contracts by k scales the payout within k − 1; call(K, S) = put(S, K), and at most one side is non-zero; on the whole `i128` domain, returns the checked result or fails exactly on overflow |
| `options_market::rounding_policy::SafeMathRounding::mul_div_{floor,ceil}` | exact floor and exact ceil; `ceil − floor` is 0 on an exact division and 1 otherwise; operands commute; `ceil(x) = −floor(−x)`; monotonic in the numerator; `mul_div(a, d, d) = a`; zero denominator is `None`; never panics on the whole `i128` domain and returns `None` on overflow, including `i128::MIN / −1` |
| `price_oracle::math::median` | bounded by the min and max; permutation invariant; equal to the value for a constant input; at least half the reports on each side; monotonic in each report; shifting every report by c (of either sign) shifts the median by c; one outlier can't move it outside the other reports' range; on the whole `i128` domain, never panics and returns exactly the round-down midpoint |
| `price_oracle::outlier_filter::aggregate` (MAD filter → median → quorum) | the price lies within `[min, max]` of the surviving reports; survivors are a subset of the input and `used + discarded` = input count; permutation invariant; a constant input aggregates to itself with nothing discarded; at least half the reports survive for k ≥ 1; monotonic in k; translation-equivariant without the band floor; quorum succeeds exactly when survivors ≥ `min_reports`; never panics for any input, `k`, band or quorum |
| `streams::math::mul_div_floor` | exact floor; operands commute; monotonic in the numerator and non-increasing in the divisor; `b <= c ⇒ result <= a`; fails exactly on overflow or a zero divisor, as documented |

The "fails exactly on overflow" properties pin down today's
panic-on-overflow behaviour of `calc_fee` and `calc_payout`.
Once those functions return typed errors, these properties should be
tightened to `Ok(correct)` or `Err(overflow)` with no panic. TWAP and
Black-Scholes helpers don't exist in this repo yet, so their suites
should come with them. Kani proofs are tracked separately.

## Deploying

```sh
soroban contract deploy \
  --wasm target/wasm32-unknown-unknown/release/zenith_options_market.wasm \
  --source <your-identity> \
  --network testnet
```

Then initialize it once, from the admin's identity:

```sh
soroban contract invoke --id <contract-id> --source <admin> --network testnet -- \
  initialize \
  --admin <admin-address> \
  --oracle <oracle-address> \
  --collateral_token <usdc-sac-address> \
  --fee_recipient <fee-recipient-address>
```

`price_oracle` deploys and initializes the same way, with just an
`--admin` argument:

```sh
soroban contract deploy \
  --wasm target/wasm32-unknown-unknown/release/zenith_price_oracle.wasm \
  --source <your-identity> \
  --network testnet

soroban contract invoke --id <contract-id> --source <admin> --network testnet -- \
  initialize --admin <admin-address>
```

## Storage TTL policy

Every contract extends its **instance** storage at the top of every
entrypoint, and extends a **persistent** entry whenever it reads or
writes it (`ttl.rs` in each crate). `extend_ttl` only charges rent once
an entry's remaining TTL drops below the threshold, so the steady-state
cost of a read is one TTL check. No contract uses temporary storage.
Ledgers are ~5s, so one day ≈ 17,280 ledgers.

| Class | Keys | Threshold | Extend to | Who pays |
|---|---|---|---|---|
| Instance | options_market: `Admin`, `Oracle`, `CollateralToken`, `FeeRecipient`, counters, `TotalPremiumsCollected`, `TotalOpenInterest`, `Paused`, `FeeRateBps`, `PremiumPool`, `ParamsRegistry`, `ParamsVersion`, `SettlementWindow`, `MaxActiveSeries`, `Migration` · price_oracle: `Admin`, `Paused`, `MaxStaleness`, `MinReports`, `Feeders` · vault: `Admin`, `Token`, `Paused`, `TotalEscrowed` · multisig: `Signers`, `Threshold`, `ApprovalTtl` · params: `Timelock`, `Version` · grants_escrow: `GrantCounter` | 23 days | 30 days | Whoever invokes any entrypoint |
| Persistent | options_market: `Series`, `Position`, `UserPositions`, `UnderlyingPrice`, `SeriesEscrow`, `SeriesCountForUnderlying`, `SeriesIndex`, `ActiveSeriesCount`, `SeriesPositions`, `SeriesClosedAt`, `SeriesReleased` · price_oracle: `PriceReport`, `AggregatedPrice` · vault: `Escrow` · multisig: `Approval` · params: `Param`, `PendingBounds` · grants_escrow: `Grant`, `Approval`, `GranteeGrants` | 60 days | 90 days | Whoever reads/writes the entry; keepers via `bump` |

Every contract also exposes a permissionless `bump(keys: Vec<DataKey>)`
that extends the instance plus each named persistent entry that exists,
so a keeper can keep long-lived but idle entries (a long-dated series,
a dormant position, an idle vault tag) from archiving. Entries that do
archive anyway can be restored — see the
[archival runbook](docs/runbooks/archival.md) and `scripts/restore/`.
In options_market, closed series and positions don't need to be kept
alive at all: once past their retention period they can be deleted
with `prune_positions` / `prune_series` (see "Pruning and the
active-series cap"), which frees the entries instead of letting them
archive with rent already paid.

## Upgrades and storage migrations

Every contract carries a `SchemaVersion` (written at `initialize`; a
missing key reads as the baseline `1`) and a `migrate(from, to)`
entrypoint that runs the storage-migration steps compiled into the
current wasm, keyed by the version they migrate **to**. Full guide,
including how to author a step: [`docs/migrations.md`](docs/migrations.md).

| Function | Description |
|---|---|
| `migrate(from, to) -> u32` | Runs steps `from+1 ..= to` in order. Rejects `from` ≠ stored version (so a step can't repeat) and `to` beyond this wasm's `CURRENT_SCHEMA_VERSION`. Returns the version reached. Who may call it depends on the contract (admin, pinned multisig, timelock, governance, any multisig signer). See the guide. |
| `migrate_batch(cursor, limit) -> u32` | Permissionless. Continues a migration whose step paginated, from exactly the cursor it stopped at (`limit` ≤ 200). |
| `schema_version()` / `migration_state()` | Stored layout version and `(target, cursor)` while a migration is in flight. |
| `upgrade(new_wasm_hash)` | options_market, price_oracle, vault: admin swaps the wasm, keeping address and storage. |
| `upgrade_via_multisig(action_id: BytesN<32>, new_wasm_hash)` | options_market, price_oracle, vault: authorized only by the trusted multisig (`set_multisig`), and only for `action_id == upgrade_action_id(new_wasm_hash)` (`sha256("zenith.upgrade" ‖ contract ‖ hash)`), so approvals bind to one wasm on one contract. `Critical` class; one-shot like every approval. |
| multisig `approve_upgrade` / `revoke_upgrade` / `upgrade` | The multisig upgrades itself only after **every** signer approves the exact hash, plus a 3-day `UPGRADE_DELAY`. Its signer set is immutable by design, and an upgrade could change it. |

While the stored version lags the code (right after an upgrade) or a
paginated migration is in flight, state-changing entrypoints fail with
`MigrationInProgress` (102). Views keep working. When upgrading through
the timelock, schedule `upgrade` and `migrate` as two calls in **one**
operation so the pair is atomic. Migration errors use the same codes in
every contract: 100 `SchemaVersionMismatch`, 101 `InvalidMigrationTarget`,
102 `MigrationInProgress`, 103 `NoMigrationInProgress`,
104 `InvalidMigrationBatch`.

## Admin functions and `Auth`

Every admin function on options_market, price_oracle and vault is a thin
entrypoint over one shared body, `x_inner(env, auth: Auth, ...)`, whose
first statement is
`zenith_common::require_admin_or_multisig(env, &DataKey::Admin, &auth, class, Error::Unauthorized)`.
Each function has two entrypoints that differ only in the `Auth` they pass:

| Entrypoint | `Auth` | Who authorizes |
|---|---|---|
| `f(args...)` | `Auth::Admin` | The stored admin's signature (`admin.require_auth()`). The admin can be a key, a multisig account, or a contract such as the timelock (which satisfies it by invoking the call itself). |
| `f_via_multisig(multisig_contract, action_id: BytesN<32>, args...)` | `Auth::Multisig(multisig_contract, action_id)` | Anyone may submit it, but `multisig_contract` must be the contract's **trusted multisig** (`set_multisig`, admin only) and must report `is_approved(action_id)`. Each approval authorizes exactly **one** call on that contract: it is recorded as used and can't be replayed for the same or any other function. |

Auth is resolved before any other state is read, so validation (caps,
bounds, pause checks) is identical on both paths — approval changes who
can call a function, never what a valid call looks like. A pause is
never bypassed by the multisig path. With no trusted multisig set, every
`_via_multisig` entrypoint fails with `Unauthorized`.

`class` (`ActionClass`) is recorded in a `multisig_approval_used(multisig,
action_id; class)` event: `Emergency` for pause/unpause and migrations,
`Standard` for configuration, `Critical` for admin transfer, upgrades,
oracle/vault re-pointing and moving escrow. The multisig exposes a single
approval tier today, so the class doesn't change whether an approval is
accepted.

**Admin transfer is two-step** on all three contracts:
`transfer_admin(new_admin)` (or its `_via_multisig` twin) only records
`new_admin` as pending (`admin_transfer_proposed` event); control moves
when `new_admin` calls `accept_admin()` (`admin_transferred` event).
Proposing the current admin cancels a pending transfer;
`get_pending_admin()` shows it. A mistyped or unusable address therefore
never strands a contract.

## `options_market` reference

All amounts are fixed-point at `PRICE_PRECISION` (1e7) unless noted —
e.g. `10_000_000` means "1.0 contracts" or "1.0 units of the underlying."
Rates (implied vol, collateral ratio, fee bps) use their own precision as
documented per field. See [Admin functions and `Auth`](#admin-functions-and-auth)
for how every admin function below is authorized.

### Admin

Each row has a `_via_multisig(multisig_contract, action_id, ...)` twin
with the same arguments after the first two, except `accept_vault_admin`.

| Function | Class | Description |
|---|---|---|
| `initialize(admin, oracle, collateral_token, fee_recipient)` | — | One-time setup (`AlreadyInitialized` on a second call). `oracle` is the settlement oracle, see `set_oracle`. |
| `set_fee_rate(new_bps)` | Standard | Protocol fee in basis points, capped at `MAX_FEE_RATE_BPS` (1000 = 10%). |
| `set_params_registry(registry)` | Standard | Points fee rate and settlement window at a `params` registry, see "`params` reference". |
| `set_oracle(oracle)` | Critical | Re-points the settlement oracle: the account that may sign `set_settlement_price`, and the only `price_oracle` contract `set_settlement_price_from_oracle` reads. |
| `set_vault(vault)` | Critical | Pins the vault that series listed from now on escrow writer collateral in. Series keep the vault they were listed with. |
| `transfer_admin(new_admin)` | Critical | Step one of the two-step transfer; `accept_admin()` completes it. |
| `pause()` / `unpause()` | Emergency | Blocks `create_series`, `update_premium`, `buy_option`, `write_option`. Never blocks exercise, settlement, reclaims or refunds — a pause winds positions down, it doesn't trap funds. |
| `create_series(underlying, option_type, strike_price, expiry, premium, implied_vol)` | Standard | Lists a series. `expiry` > 1 hour out, `strike_price` and `premium` > 0, `implied_vol` >= 0 (`InvalidSeriesParams`). At most `MAX_SERIES_PER_UNDERLYING` (50) per underlying (`TooManySeriesForUnderlying`). A series listed while a vault is pinned is **vault-backed**, see below. |
| `update_premium(series_id, new_premium, new_implied_vol)` | Standard | Re-prices an Active series; `new_premium` > 0. |
| `cancel_series(series_id)` | Standard | Cancels an Active series; holders then claim refunds individually. |
| `accept_vault_admin(vault)` | Critical (admin only) | Accepts a pending admin transfer of `vault` to this contract, since only this contract can authorize the vault's `accept_admin`. |
| `upgrade(new_wasm_hash)` / `upgrade_via_multisig(action_id, new_wasm_hash)` | Critical | See [Upgrades](#upgrades-and-storage-migrations). |
| `set_multisig(multisig)` / `migrate(from, to)` | Critical / — | Trusted multisig and schema migration, same as every contract. |

### Settlement

| Function | Description |
|---|---|
| `set_settlement_price(series_id, price)` | The trusted oracle **account** signs a price after expiry. |
| `set_settlement_price_from_oracle(series_id, oracle_contract)` | Permissionless: anyone settles an expired series from the configured `price_oracle`'s `get_price` (freshness → outlier filter → median → quorum). `oracle_contract` must equal the configured oracle (`Unauthorized` otherwise), so a caller can't settle against an oracle of their choosing. `PriceNotSet` when the oracle has no quorum-backed price. |

Both are one-shot: `AlreadySettled` for a Settled series, `SeriesNotActive`
for a Cancelled one, `SeriesNotExpired` before expiry, `PriceNotSet` for a
non-positive price.

### Traders

| Function | Description |
|---|---|
| `buy_option(buyer, series_id, contracts, max_premium) -> position_id` | Opens a Long. `max_premium` is slippage protection. The fee goes to `fee_recipient`; the rest funds the premium pool writers are paid from. |
| `write_option(writer, series_id, contracts, collateral_amount, min_premium) -> position_id` | Opens a Short. Collateral: notional at the last settlement price of the underlying (or the strike) for calls, 110% of strike notional for puts. The writer's premium (net of fee) comes out of the pool buyers funded — `InsufficientPremiumPool` if it can't cover it, `PremiumBelowMinimum` below `min_premium` (pass `0` to opt out). |
| `exercise(owner, position_id) -> payout` / `exercise_batch(owner, ids)` | In-the-money longs after expiry, within the settlement window (24h, or the registry's `settle_w`). Batches are all-or-nothing, up to `MAX_BATCH_SIZE` (25). |
| `reclaim_collateral(writer, position_id) -> amount` / `reclaim_batch(writer, ids)` | After settlement: collateral minus what the position's contracts owe at the settlement price. |
| `claim_refund(owner, position_id)` | Cancelled series: longs get premium back net of the fee they paid; shorts get their full collateral. |
| `escrow_series_to_vault(vault_contract, series_id)` | Permissionless, Cancelled series only: moves the refund liability still held in this contract's own balance into the series' vault (or the pinned vault for a series without one; `UnauthorizedVault` for any other) under tag `series_id`, then checks the tag grew by exactly that much. |
| `claim_refund_from_vault(vault_contract, owner, position_id)` | `claim_refund`, paid from the series' vault tag. `vault_contract` must be the vault recorded for the series. |

### Vault-backed series

A series listed while a vault is pinned keeps that vault for life
(`get_series_vault`, `is_vault_backed`). Every `write_option` on it
deposits the writer's collateral into the vault under tag `series_id`
(the market authorizes the vault's nested token pull explicitly), and
every exercise payout, reclaim and short-side refund is withdrawn from
that tag only. A payout a series' own collateral can't cover fails with
the vault's `InsufficientEscrowBalance` — it never draws on another
series' collateral. Series listed with no vault pinned keep collateral in
the market's own balance, as before. In the protocol's deployment the
vault's admin is the market (handed over two-step with
`accept_vault_admin`).

### Views

`get_admin`, `get_pending_admin`, `get_oracle`, `get_vault`,
`get_series_vault(series_id)`, `is_vault_backed(series_id)`, `is_paused`,
`get_fee_rate`, `get_settlement_window`, `get_premium_pool`,
`get_series_count_for_underlying`, `get_series`, `get_position`,
`get_user_positions`, `get_underlying_price`, `get_series_escrow`
(refund liability still in the market's own balance), `get_stats`
(premiums collected net of fees, open interest, series count),
`get_multisig`, `upgrade_action_id(hash)`, `schema_version`,
`migration_state`.

### Errors

| # | Error | | # | Error |
|---|---|---|---|---|
| 1 | `AlreadyInitialized` | | 19 | `InvalidFeeRate` |
| 2 | `Unauthorized` | | 20 | `TooManySeriesForUnderlying` |
| 3 | `SeriesNotFound` | | 21 | `InsufficientPremiumPool` |
| 4 | `SeriesNotActive` | | 22 | `InvalidSeriesParams` |
| 5 | `SeriesNotExpired` | | 23 | `InvalidBatchSize` |
| 6 | `PositionNotFound` | | 24 | `NothingToEscrow` |
| 7 | `InsufficientPremium` | | 25 | `PremiumBelowMinimum` |
| 8 | `InsufficientCollateral` | | 35 | `UnauthorizedVault` |
| 9 | `AlreadyExercised` | | 36 | `NoPendingAdmin` |
| 10 | `AlreadySettled` | | 100–104 | migration errors |
| 11 | `ExerciseWindowClosed` | | | |
| 12 | `ZeroContracts` | | | |
| 13 | `PriceNotSet` | | | |
| 14 | `NotInTheMoney` | | | |
| 15 | `WrongSide` | | | |
| 16 | `ExpiryTooSoon` | | | |
| 17 | `ContractPaused` | | | |
| 18 | `SeriesNotCancelled` | | | |

Codes 26–34 are reserved by entrypoints (forfeiture, duplicate series,
splitting, pruning, pagination) that are not part of this build.

## `price_oracle` reference

### Aggregation

`get_price(symbol)` and `aggregate(symbol)` combine feeder reports in
this order, implemented as pure functions over a slice of prices in
`price_oracle/src/outlier_filter.rs` and `price_oracle/src/math.rs`:

1. **Freshness.** Only reports from currently authorized feeders that are
   at most `max_staleness` seconds old count (admin-settable, default one
   hour). This runs first, so a stale report can neither count toward the
   quorum nor skew the filter.
2. **Outlier filter** (median absolute deviation). With `center` = median
   of the fresh reports and `mad` = median of `|report − center|`, a
   report survives if `|report − center| <= max(3 × mad, |center| × 1%)`.
   The 1% floor keeps honest reports when most feeders agree exactly
   (`mad` = 0). With fewer than 3 fresh reports nothing is filtered —
   there is no majority to say which one is the outlier, and the quorum
   below covers that case. With 3 or more, at least half the reports
   (rounded up) always survive.
3. **Median** of the surviving reports. For an even count it is the
   midpoint of the two middle reports **rounded down** (toward negative
   infinity); e.g. `[10, 21]` → `15`. Computed as `lo + (hi − lo) / 2` in
   unsigned arithmetic, so it cannot overflow, and it always lies within
   `[min, max]` of the surviving reports.
4. **Quorum.** At least `min_reports` reports must survive the filter
   (admin-settable, default 1, at most `MAX_FEEDERS`). Otherwise there is
   no price: `get_price` returns `None` and `aggregate` fails with
   `QuorumNotMet`. A feeder set that has mostly gone stale or been removed
   therefore fails closed instead of degrading to a single reporter.

All arithmetic is integer-only on the 7-decimal fixed-point prices; no
step panics on any report values.

### Admin

Every row except `initialize` has a `_via_multisig(multisig_contract,
action_id, ...)` twin; see [Admin functions and `Auth`](#admin-functions-and-auth).

| Function | Class | Description |
|---|---|---|
| `initialize(admin)` | — | One-time setup. `max_staleness` = 3600, `min_reports` = 1. |
| `set_max_staleness(seconds)` | Standard | Freshness window. Rejects zero (`InvalidStaleness`). |
| `set_min_reports(count)` | Standard | Quorum of surviving fresh reports. `1..=MAX_FEEDERS` (`InvalidMinReports`). |
| `add_feeder(feeder)` / `remove_feeder(feeder)` | Standard | Authorize/revoke a reporter; at most `MAX_FEEDERS` (16). A removed feeder's past reports stay readable via `get_latest_report` but no longer count. Blocked while paused. |
| `transfer_admin(new_admin)` | Critical | Two-step; `accept_admin()` completes it. |
| `pause()` / `unpause()` | Emergency | Blocks feeder changes, `report_price` and `aggregate`. `get_price` keeps working. |
| `upgrade` / `upgrade_via_multisig` / `set_multisig` / `migrate` | Critical | Same as every contract. |

### Feeders and keepers

| Function | Description |
|---|---|
| `report_price(feeder, symbol, price)` | Records the feeder's own observation (feeder signature; positive prices only). |
| `aggregate(symbol) -> PriceRound` | Permissionless. Runs the pipeline above and records the result as the symbol's next round `{ round_id, price, timestamp, reports_used, reports_discarded }` (event `round_aggregated`). Fails with `NoFreshReports` or `QuorumNotMet` and writes nothing otherwise. |

### Views

`get_price(symbol) -> Option<i128>` (the pipeline above, as a read — what
options_market cross-calls), `get_latest_round(symbol)`,
`get_latest_report(symbol, feeder)` (raw, regardless of freshness),
`is_feeder`, `get_feeders`, `get_feeder_count`, `get_max_staleness`,
`get_min_reports`, `get_admin`, `get_pending_admin`, `is_paused`,
`get_multisig`.

### Events

`admin_transfer_proposed`, `admin_transferred`, `paused`, `unpaused`,
`max_staleness_updated`, `min_reports_updated`, `feeder_added`,
`feeder_removed`, `price_reported`, `round_aggregated` (topics
`symbol`; data `(round_id, price, reports_used, reports_discarded)`),
`multisig_approval_used`.

### Errors

| # | Error | | # | Error |
|---|---|---|---|---|
| 1 | `AlreadyInitialized` | | 8 | `TooManyFeeders` |
| 2 | `FeederAlreadyAdded` | | 9 | `Unauthorized` |
| 3 | `FeederNotFound` | | 10 | `InvalidMinReports` |
| 4 | `NotAFeeder` | | 11 | `NoFreshReports` |
| 5 | `InvalidPrice` | | 12 | `QuorumNotMet` |
| 6 | `ContractPaused` | | 13 | `NoPendingAdmin` |
| 7 | `InvalidStaleness` | | 100–104 | migration errors |

## `vault` reference

A per-tag escrow ledger for a single token. `tag` is a caller-defined
`u64` (options_market uses its `series_id`); a withdrawal or tag transfer
can never take more than the tag's own balance, whatever the vault's raw
token balance is.

### Admin

Every row except `initialize` has a `_via_multisig(multisig_contract,
action_id, ...)` twin; see [Admin functions and `Auth`](#admin-functions-and-auth).
In the protocol's deployment the admin is options_market.

| Function | Class | Description |
|---|---|---|
| `initialize(admin, token)` | — | One-time setup. |
| `withdraw(tag, to, amount)` | Critical | Pays `amount` of `tag`'s escrow to `to`; `InsufficientEscrowBalance` if the tag holds less. |
| `transfer_tag(from_tag, to_tag, amount)` | Critical | Re-earmarks escrow between tags; no tokens move. |
| `sweep_untagged(to)` | Standard | Recovers tokens sent to the vault without `deposit` (balance above the ledger total). |
| `transfer_admin(new_admin)` | Critical | Two-step; `accept_admin()` completes it. |
| `pause()` / `unpause()` | Emergency | Blocks `deposit`, `withdraw`, `transfer_tag`. |
| `upgrade` / `upgrade_via_multisig` / `set_multisig` / `migrate` | Critical | Same as every contract. Set the trusted multisig **before** handing the admin to options_market. |

### Depositors

| Function | Description |
|---|---|
| `deposit(from, tag, amount)` | Pulls `amount` from `from` (signature required) and credits `tag`. The vault measures its own balance before and after; if less than `amount` arrived (fee-on-transfer, a no-op transfer, a lying `balance`), the deposit fails with `DeltaMismatch`. |

### Views

`balance_of(tag)`, `get_total_escrowed`, `get_admin`, `get_pending_admin`,
`get_token`, `is_paused`, `get_multisig`.

### Events

`deposited` / `withdrawn` (topics `from`/`to`, `tag`; data `amount`),
`tag_transferred` (topics `from_tag`, `to_tag`; data `amount`),
`swept_untagged`, `admin_transfer_proposed`, `admin_transferred`,
`paused`, `unpaused`, `multisig_approval_used`.

### Errors

1 `AlreadyInitialized`, 2 `InvalidAmount`, 3 `InsufficientEscrowBalance`,
4 `ContractPaused`, 5 `NoUntaggedFunds`, 6 `Unauthorized`,
7 `DeltaMismatch`, 8 `NoPendingAdmin`, 100–104 migration errors.

## `multisig` reference

Every signer has a weight and `threshold` is a total weight: an action
is approved once the summed weight of its fresh approvals reaches it.
The legacy `initialize` gives every signer weight 1, which is plain
M-of-N. `approval_ttl` is fixed at initialization.

> **Note:** the timelock tiers (`is_executable`, `delays`) and custom
> account (`__check_auth`) documented below were described by #150, but
> their source was never committed. The multisig here was restored from
> #149 and does not implement them yet. `zenith-common` calls the
> multisig's `is_approved(action_id)`, which it does implement.

### Why rotation exists, and how it stays safe

Signers used to be immutable, so that a compromised signer could never
add another compromised signer. The cost was liveness: every lost key
permanently weakened the multisig, and losing `N − M + 1` keys bricked
every contract it administers. Redeploying doesn't help, because
re-pinning a new multisig needs the old one's approval.

Rotation keeps the original safety goal with four guards:

1. **Higher bar.** A signer change needs `rotation_threshold` approval
   weight, which is always above `threshold` (or equal to the total
   weight, i.e. unanimity). A normal quorum, including one holding a
   compromised key, can't rotate on its own.
2. **Mandatory delay with a single-signer veto.** Once approved, a change
   waits `rotation_delay` seconds before it can execute. Any one signer
   can `veto_signer_change` until then, so a single honest key can stop
   a hostile rotation.
3. **Small steps.** At most one signer is added and one removed per
   change. Every invariant (`1 <= threshold <= total weight`, no
   duplicates, weights > 0, overflow-safe sums, a reachable rotation
   threshold) is checked when the change is proposed and again when it
   executes.
4. **Epoch invalidation.** Approvals are stored as
   `Approval(epoch, action, signer)`. Executing a change bumps
   `SignerEpoch`, which invalidates every outstanding vote in O(1),
   including votes from a removed signer. A change proposed in an
   earlier epoch can't execute (`StaleSignerChange`).

Action ids are `BytesN<32>`. Every action has an on-chain registry entry,
`ActionMeta { proposer, created_at, description_hash, status }`
(`status`: `Pending`, `Executed`, `Reset`, `Expired`), created either by
`register_action` or on its first approval. Pending actions live in a
bounded index (at most `MAX_PENDING_ACTIONS` (100) overall and
`MAX_PENDING_PER_SIGNER` (10) per proposer, against registry spam) that
is swap-removed on execute, reset, or expiry.

| Function | Description |
|---|---|
| `initialize(signers, threshold, approval_ttl)` | Backward-compatible equal-weight setup: every signer gets weight 1. The rotation threshold is `threshold + 1` (or unanimity when `threshold` is already every signer), and the rotation delay is `DEFAULT_ROTATION_DELAY` (3 days). Rejects a zero threshold, a threshold above the signer count, or a duplicate signer. `approval_ttl` is in seconds; zero means approvals never expire. |
| `initialize_weighted(signers: Vec<(Address, u32)>, threshold_weight, approval_ttl, rotation_threshold, rotation_delay)` | Weighted setup. Rejects a zero weight (`InvalidWeight`), a total weight that overflows `u32` (`WeightOverflow`), duplicates, `threshold_weight` outside `1..=total`, and a rotation threshold that isn't above the threshold (or equal to the total weight) or is above the total. A signer whose weight alone meets the threshold is **allowed** but announced with a `single_key_quorum` event, e.g. for a deliberate hardware-secured cold key. |
| `register_action(proposer, action_id, description_hash)` | Signer-only. Registers `action_id` with a `description_hash` (e.g. sha256 of an off-chain write-up) before anyone votes, so signers can verify on-chain what they approve. Rejects an id that's already pending or executed. |
| `approve(signer, action_id)` | Records `signer`'s approval, stamped with the current ledger timestamp. Requires the signer's own signature and current signer-set membership. Registers the action (zero description hash) if it isn't pending yet. Rejects an executed action, and a signer voting twice while their existing approval is still fresh — an EXPIRED approval is treated as no approval at all, so re-approving after expiry just refreshes the timestamp. |
| `revoke(signer, action_id)` | Withdraws `signer`'s own still-fresh vote. Rejects revoking an approval that's already expired — there's nothing left to withdraw. |
| `reset(action_id)` | Clears every signer's approval of `action_id` and marks it `Reset` (removed from the pending index), so a repeat action reusing the same id starts from a clean slate. Only callable once `action_id` is already approved; not for executor proposals. |
| `expire(action_id)` | Anyone. Marks a stale pending action `Expired` and drops it from the index: requires a nonzero `approval_ttl`, the action older than it, and no approval still fresh. |
| `propose(proposer, target, function, args) -> proposal_id` | Signer-only. Stores `Proposal { target, function, args, nonce }` (at most `MAX_PROPOSAL_ARGS` (10) args) and registers it. `proposal_id` = sha256 of the proposal's XDR, which is also its description hash, so the id commits to the exact call. |
| `execute(proposal_id)` | Anyone, once approved. Marks the proposal `Executed`, then calls `target.function(args)` via `invoke_contract` **as the multisig** — so a target whose admin is the multisig passes its `admin.require_auth()` with no other signature. Can never run twice; a failing target call reverts the whole transaction (the proposal stays pending). Returns the target's return value. |
| `propose_signer_change(proposer, add: Vec<SignerWeight>, remove: Vec<Address>, new_threshold, new_rotation_threshold) -> change_id` | Signer-only. `add` and `remove` each hold at most one entry; adding the address being removed changes its weight. Validated against the resulting set. `change_id` = sha256 of the `SignerChange` XDR (including the current epoch and a nonce), so approvals bind to the exact payload. Emits `signer_change_proposed`. |
| `queue_signer_change(change_id) -> ready_at` | Anyone, once `get_approval_weight(change_id) >= rotation_threshold`. Starts the `rotation_delay`. |
| `veto_signer_change(signer, change_id)` | Any single signer, any time before execution. Marks the change `Reset` and emits `signer_change_vetoed`. |
| `execute_signer_change(change_id)` | Anyone, at or after `ready_at`, as long as the approval weight still meets `rotation_threshold` and the epoch hasn't changed. Applies the change, bumps the epoch and emits `signers_rotated`. |

Weighted example: two core members (weight 2) and three community members (weight 1) with `threshold_weight = 4`. Any 2 core members approve (2 + 2), or 1 core member plus 2 community members (2 + 1 + 1), but 3 community members alone (3) do not.

```sh
stellar contract invoke --id $MULTISIG -- initialize_weighted \
  --signers '[["'$CORE_A'",2],["'$CORE_B'",2],["'$COMM_A'",1],["'$COMM_B'",1],["'$COMM_C'",1]]' \
  --threshold_weight 4 --approval_ttl 0 --rotation_threshold 6 --rotation_delay 259200
```

Rotating out a lost key (2-of-3 legacy setup, rotation threshold 3):

```sh
ID=$(stellar contract invoke --id $MULTISIG --source a -- propose_signer_change \
  --proposer $A --add '[{"address":"'$D'","weight":1}]' --remove '["'$C'"]' \
  --new_threshold 2 --new_rotation_threshold 3)
# a, b, c approve($ID) -> queue_signer_change($ID) -> wait 3 days -> execute_signer_change($ID)
```

A lost key can't vote, so the rotation threshold must stay reachable
without it. Set it below the total weight when you initialize.

### Views

`is_signer`, `get_signers` (address → weight), `get_signer_weight(address)` (0 if not a signer), `get_signer_count`, `get_total_weight`, `get_threshold` (a weight), `get_rotation_threshold`, `get_rotation_delay`, `get_signer_epoch`, `get_approval_ttl`, `has_approved(action_id, signer)` (false if expired or from an earlier epoch), `get_approval_count(action_id)` (number of signers with a fresh approval), `get_approval_weight(action_id)` (their summed weight), `is_approved(action_id)` (weight ≥ threshold), `get_signer_change(change_id)`, `get_signer_change_ready_at(change_id)`, `get_action(action_id)` (`ActionMeta` or none), `get_pending_actions(cursor, limit)` (up to `limit` ≤ 50 pending ids from index `cursor`; order isn't stable across writes because removal is swap-remove), `get_pending_count`, `get_proposal(proposal_id)`.

### Timelock tiers

Every `_via_multisig` entrypoint declares an `ActionClass`, and checks
`is_executable(action_id, class)`: the action must be approved **and**
have stayed at threshold for that class's delay. The approval that
crosses the threshold records `threshold_reached_at`; a revoke that
drops below clears it, and if an approval expires during the delay the
action drops below threshold and the approval that restores it restarts
the timer. `reset` clears it too. Delays are immutable (set at
`initialize`). Admin-signed (non-multisig) calls are not timelocked.

| Class | Delay | Functions |
|---|---|---|
| `Emergency` | none | `pause_via_multisig` (all three contracts) |
| `Standard` | `delays.standard` (e.g. 24h) | `unpause_via_multisig` (all); options_market `set_fee_rate`, `create_series`, `update_premium`, `cancel_series`; price_oracle `set_max_staleness`, `set_min_reports`, `add_feeder`, `remove_feeder` (all `_via_multisig`) |
| `Critical` | `delays.critical` (e.g. 72h) | `transfer_admin_via_multisig` (all); options_market `upgrade`; vault `withdraw`, `transfer_tag`, `sweep_untagged` (all `_via_multisig`) |

### Custom account (`__check_auth`)

With an `AccountConfig`, the multisig's own address works as a plain
`admin: Address` in any contract: every existing `admin.require_auth()`
is satisfied by a transaction auth entry carrying
`Vec<AccountSignature { public_key, signature }>` — at least
`threshold` ed25519 signatures over the auth payload from distinct
configured keys, **sorted strictly ascending by public key** (duplicates
and unsorted lists are rejected, as are non-signer keys). If
`allowed_contracts` is non-empty, the account only authorizes calls into
those contracts and never contract creation.

**Which mode to use:** custom-account mode needs no `_via_multisig`
code, works with every admin-gated function (and can hold funds and sign
token transfers), but signatures are collected off-chain and there is
**no timelock**. Approval mode (`approve` + `_via_multisig`) is
on-chain, auditable vote-by-vote, and enforces the timelock tiers above
— prefer it for upgrades and admin transfers.

CLI walkthrough for signing an admin transaction (here `pause` on
options_market whose `admin` is the multisig account):

```sh
# 1. Build + simulate the call; the simulation returns the auth entry
#    the multisig address must sign.
stellar contract invoke --id <market-id> --source <fee-payer> --network testnet \
  --build-only -- pause > pause.tx
stellar tx simulate --network testnet < pause.tx > pause.sim.tx
```

```js
// 2. Each signer signs the entry's payload; assemble sorted signatures.
import { xdr, hash, Keypair } from "@stellar/stellar-sdk";
const creds = entry.credentials().address(); // entry: the simulated SorobanAuthorizationEntry
creds.signatureExpirationLedger(validUntil);
const payload = hash(xdr.HashIdPreimage.envelopeTypeSorobanAuthorization(
  new xdr.HashIdPreimageSorobanAuthorization({
    networkId: hash(Buffer.from(networkPassphrase)),
    nonce: creds.nonce(),
    signatureExpirationLedger: validUntil,
    invocation: entry.rootInvocation(),
  })).toXDR());
const sigs = signers // Keypair[] holding M of the configured keys
  .sort((a, b) => Buffer.compare(a.rawPublicKey(), b.rawPublicKey()))
  .map((kp) => xdr.ScVal.scvMap([
    new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol("public_key"), val: xdr.ScVal.scvBytes(kp.rawPublicKey()) }),
    new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol("signature"), val: xdr.ScVal.scvBytes(kp.sign(payload)) }),
  ]));
creds.signature(xdr.ScVal.scvVec(sigs));
```

```sh
# 3. Re-simulate with the signed entry (for accurate fees), sign as the
#    fee payer, and submit.
stellar tx simulate --network testnet < pause.signed.tx \
  | stellar tx sign --sign-with-key <fee-payer> --network testnet \
  | stellar tx send --network testnet
```

### Events

`registered`, `approved`, `revoked` (topics: name, address, action id), `reset`, `expired`, `executed` (topics: name, action id). Every event's data is the action's description hash. Rotation: `signer_change_proposed` (proposer, change id; data: the `SignerChange`), `signer_change_queued` (change id; data: ready-at), `signer_change_vetoed` (signer; data: change id), `signers_rotated` (change id; data: new epoch). `single_key_quorum` (signer; data: `(weight, threshold)`) warns that one signer can approve alone.

### Errors

| # | Error |
|---|---|
| 1 | `AlreadyInitialized` |
| 2 | `InvalidThreshold` |
| 3 | `DuplicateSigner` |
| 4 | `NotASigner` |
| 5 | `AlreadyApproved` |
| 6 | `NotYetApproved` |
| 7 | `InvalidDelays` |
| 8 | `AccountNotConfigured` |
| 9 | `UnsortedSignatures` |
| 10 | `InsufficientSignatures` |
| 11 | `ContextNotAllowed` |
| 12 | `ActionAlreadyRegistered` |
| 13 | `ActionNotPending` |
| 14 | `TooManyPendingActions` |
| 15 | `ProposalNotFound` |
| 16 | `ProposalTooLarge` |
| 17 | `InvalidPageLimit` |
| 18 | `NotExpired` |
| 19 | `InvalidWeight` |
| 20 | `WeightOverflow` |
| 21 | `InvalidSignerChange` |
| 22 | `SignerChangeNotFound` |
| 23 | `StaleSignerChange` |
| 24 | `RotationDelayActive` |
| 25 | `SignerChangeNotQueued` |

### Approval storage, veto and deadlines, pruning

**Single-entry approvals.** Every action's approvals live in one entry,
`Approvals(epoch, action_id) -> { bitmap: u32, timestamps: Vec<u64> }`, indexed
by the signer's position in the signer map, so `is_approved`,
`has_approved`, `get_approval_count`, `approve`, `revoke` and `reset` each touch
one approvals entry instead of one per signer. At most 32 signers are
allowed (`TooManySigners`). A signer-set rotation bumps the epoch, which
starts a fresh index space. Expired approvals are detected by comparing
the stored timestamps. Entries written by older deployments (one
`Approval(epoch, id, signer)` entry per signer) are not read; such a
deployment must be re-approved or upgraded with a migration.

| `is_approved`, 16 signers, all approved | before | after |
|---|---|---|
| approval ledger entries read | 16 | 1 |
| CPU instructions | 887,941 | 136,629 |

(`bench_is_approved_16_signers`; the deadline entry is read only once quorum
is met.)

**Storage class.** With `approval_ttl > 0` the approvals entry is
*temporary* storage whose TTL covers `approval_ttl` (assuming ledgers close at
least every 3 s, i.e. it can only outlive the approvals). When it lapses every
approval in it has already expired, which is exactly "no approvals".
With `approval_ttl == 0` approvals never expire and stay persistent. The
action registry entry (`Action(id)`, which carries the executed flag used as
replay protection) is persistent, never pruned, and re-extended to the standard
persistent TTL when an action finishes; anyone can keep it alive with `bump`.

**`prune(action_ids)`** is permissionless and bounded (`MAX_PRUNE` = 20). It
deletes the approvals entry of an action only if that cannot change
`is_approved`: the action is finished, or it has no fresh approval and no passed
deadline. Registry entries and deadlines are never removed. Nothing prunes
automatically.

**Veto and deadlines.** `set_vetoer` can only be called by the multisig itself
(through an approved `propose`/`execute`), exactly once; the vetoer is then
immutable (replacing it means redeploying). `veto(vetoer, id)` cancels a
*pending* action permanently: approvals are cleared, `approve` rejects it, and
`is_approved` stays false; a veto after execution is rejected, and the
vetoer may also be a signer. The proposer of a pending action may call
`set_execute_by(id, ts)` once; after `ts` (inclusive boundary) `is_approved`
is false regardless of `approval_ttl`, and `expire` retires it with an
`action_expired` event. `veto` emits `vetoed`.

```
register ──approve──▶ Pending ──threshold──▶ is_approved ──execute/reset──▶ Executed/Reset
              │          │                        │
              │          ├─ veto (vetoer) ────────┴──▶ Vetoed   (terminal, never approved again)
              │          └─ now > execute_by ─ expire ▶ Expired (action_expired)
              └─ approval older than approval_ttl stops counting (temporary entry lapses)
```

### Migrating from `_via_multisig` to executor mode

1. Deploy and `initialize` a multisig with the intended signers and threshold.
2. On each target contract, `transfer_admin(<multisig address>)` — from then on the multisig is the admin, and every admin function keeps its single `admin.require_auth()` path.
3. For each admin action: a signer calls `propose(signer, target, fn_name, args)`, signers `approve(signer, proposal_id)` until threshold, then anyone calls `execute(proposal_id)`.
4. The existing `_via_multisig` entrypoints and `is_approved` keep working unchanged (with `BytesN<32>` action ids), so consumers can move over one function at a time; their removal is a follow-up deprecation.

## `timelock` reference

Holds the admin role on options_market, price_oracle and vault-like
contracts, so every admin change is visible on-chain for at least
`min_delay` seconds before it can run. The consumer contracts don't know
their admin is a timelock: it satisfies `admin.require_auth()` by being
the direct invoker of the calls it executes. Hand-off runbook:
[docs/runbooks/timelock-handoff.md](docs/runbooks/timelock-handoff.md).

| Role | Who | Can |
|---|---|---|
| Proposer | one address (multisig account, later the governor) | `schedule`, `schedule_after`, `cancel` |
| Executor | anyone, unless `set_exec(Some(addr))` restricts it | `execute` a ready operation |
| Canceller | the proposer, the guardian (until lock-in), and addresses granted with `set_canc` | `cancel` a pending operation |
| Guardian | set at `initialize`, removed by `lock_in` | `cancel`, `guardian_set_proposer` (immediate rollback) |

| Function | Description |
|---|---|
| `initialize(proposer, guardian, min_delay, grace_period)` | One-time setup. |
| `schedule(op_id, targets, fns, args, delay) -> ready_at` | Proposer only. A batch of calls, executable from `now + delay` (`delay >= min_delay`) until `ready_at + grace_period`. |
| `schedule_after(op_id, predecessor, targets, fns, args, delay)` | Same, but only executable after `predecessor` has executed. The predecessor must already exist, so dependencies always point back in time and can't cycle; a cancelled predecessor only blocks its own dependents. |
| `execute(op_id)` | Runs every call in order; any revert reverts the whole operation. `NotReady` before `ready_at`, `Expired` after the grace period, `PredecessorNotDone`. |
| `cancel(caller, op_id)` | A canceller removes a pending operation. |
| `guardian_set_proposer(new_proposer)` | Guardian only, until lock-in. |

**Self-calls.** Soroban forbids re-entry, so an operation targeting the
timelock itself is applied in place by `execute`: `set_prop(Address)`,
`set_delay(u64)`, `set_canc(Address, bool)`, `set_exec(Option<Address>)`,
`lock_in()`, `migrate(u32, u32)`. Every role change is therefore delayed.

**Clock.** Every delay is a ledger-**timestamp** delay — the same clock the
governor's proposal `eta` uses — never a ledger count.

**Views:** `get_operation`, `is_pending`, `is_ready`, `is_done`,
`get_proposer`, `get_guardian`, `is_locked_in`, `is_canceller`,
`get_executor`, `get_min_delay`, `get_grace_period`, `schema_version`.

**Events:** `scheduled` (topics `op_id`; data `(ready_at, predecessors)`),
`executed`, `canceled`, `proposer_set`, `canceller_set`, `executor_set`,
`min_delay_set`, `locked_in`.

**Errors:** 1 `AlreadyInitialized`, 2 `Unauthorized`, 3 `InvalidOperation`,
4 `DelayTooShort`, 5 `OperationExists`, 6 `OperationNotFound`,
7 `NotReady`, 8 `Expired`, 9 `AlreadyDone`, 10 `LockedIn`,
11 `UnknownSelfCall`, 12 `PredecessorNotFound`, 13 `PredecessorNotDone`,
100–104 migration errors.

## `params` reference

All writes require the `timelock` address set at `initialize`. Values
move freely inside their bounds; bounds themselves only move through a
second, slower path (`BOUNDS_CHANGE_DELAY` = 7 days on top of the
timelock's own delay), so widening the sanity envelope always takes
longer than using it. Every change bumps `get_version()`.

| Function | Description |
|---|---|
| `initialize(timelock)` | One-time setup. |
| `define_param(key, value, min, max)` | Creates a parameter that has never been set. |
| `set_param(key, value)` | Moves the value within `[min, max]`. Emits `param_updated(key, old, new)`. |
| `propose_bounds(key, min, max)` | Queues new bounds (must still contain the current value), executable after `BOUNDS_CHANGE_DELAY`. |
| `execute_bounds(key)` / `cancel_bounds(key)` | Applies or drops the queued bounds change. |

Views: `get_param(key) -> Option<{value, min, max, updated_at}>`,
`get_value(key)`, `get_pending_bounds(key)`, `get_version()`,
`get_timelock()`. Events: `param_defined`, `param_updated`,
`bounds_proposed`, `bounds_updated`, `bounds_cancelled`.

### Parameter catalog

| Key | Consumer | Unit | Suggested bounds | Default when unset |
|---|---|---|---|---|
| `fee_bps` | options_market fee rate | basis points | `[0, 1000]` | local `FeeRateBps` (50) |
| `settle_w` | options_market exercise window after expiry | seconds | `[3600, 604800]` | 86,400 |

options_market points at a registry via admin-only
`set_params_registry(registry)`. It caches both values in instance
storage with the registry version and only re-reads them when the
version moves. An unreachable registry, an unset key, or a value
outside options_market's own hard limits (`MAX_FEE_RATE_BPS`, window > 0)
leaves the cached value in place. Other parameters (oracle staleness,
`min_reports`, OI caps, ...) are follow-ups.

## `grants_escrow` reference

| Function | Description |
|---|---|
| `create_grant(funder, grantee, token, milestones: Vec<(amount, deadline)>, reviewers, quorum) -> u64` | Funds the grant upfront. Rejects a reviewer who is also the grantee, duplicate reviewers, a quorum outside `1..=reviewers`, and non-positive amounts or past deadlines. |
| `submit_milestone(grant_id, idx, evidence_hash)` | Grantee only, before the milestone's deadline. |
| `approve_milestone(grant_id, idx, reviewer)` | One approval per reviewer; the approval that reaches quorum releases the milestone in full (no partial approval). |
| `reclaim(grant_id) -> i128` | Funder only. Returns every unreleased milestone whose deadline plus `GRACE_PERIOD` (7 days) has passed. |

Views: `get_grant`, `get_grant_count`, `has_approved`,
`get_grantee_grant_count(grantee)`, `get_grants_by_grantee(grantee,
start, limit)` (paginated, `limit` capped at 50). Events:
`grant_created`, `milestone_submitted`, `milestone_approved`,
`milestone_released`, `funds_reclaimed`.

## Known gaps

- **No order matching.** This is a pooled market, not an order book —
  writers and buyers aren't matched 1:1, and the premium pool
  (`PremiumPool`) is shared across every series, so `write_option`'s
  `InsufficientPremiumPool` check is a contract-wide constraint. A
  vault-backed series' collateral only covers payouts up to what its own
  writers locked: if a series has more long contracts than short ones,
  the excess longs' payouts fail (`InsufficientEscrowBalance`) instead of
  being paid out of other series' collateral.
- **Premium refunds on cancellation still come from the pooled
  balance.** `SeriesEscrow` tracks each cancelled series' outstanding
  premium refunds and `escrow_series_to_vault` moves them into the
  series' vault tag, but premium a buyer contributed may already have
  been paid to a writer (possibly in another series). Writer collateral
  of vault-backed series is fully isolated per series.
- **Covered-call collateral is notional at listing price.** A call that
  finishes far enough in the money can owe more than its writer locked;
  `reclaim_collateral` floors at zero and the long's payout fails rather
  than drawing on other funds (see `call_collateral.rs`, not wired in).
- **Token deltas.** The vault rejects deposits whose measured balance
  delta falls short; options_market itself still records premiums and
  collateral at the requested amount (#115 tests are `#[ignore]`d).
- **Approval classes are informational.** `require_admin_or_multisig`
  records each consumed approval's `ActionClass`, but the multisig has a
  single approval tier until its timelock tiers are rebuilt; use the
  timelock as admin for delayed changes.
- **Not part of this build:** options_market forfeiture sweeps, position
  transfer/splitting, pruning and pagination (their error codes 26–34
  are reserved), and the resource-snapshot suite in
  `options_market/src/test_resources.rs`, which targets those
  entrypoints.
