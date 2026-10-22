# Zenith Contracts

Soroban (Stellar smart contract) crates for the Zenith options protocol.

## Crates

- [`options_market/`](options_market) — European-style put/call options on
  XLM, BTC, ETH, and SOL. Premium is set by the admin (computed off-chain via
  Black-Scholes); writers lock collateral, buyers pay premium, settlement
  happens at expiry against an oracle-reported price.
- [`price_oracle/`](price_oracle) — the on-chain logic behind that
  oracle-reported price. options_market previously only ever trusted a
  bare `oracle: Address` with nothing backing it; this contract is a
  small set of admin-authorized feeders reporting per-symbol prices,
  aggregated as a median across whatever's still fresh, gated by an
  admin-settable minimum quorum of fresh reports (`min_reports`) so a
  single feeder can't single-handedly determine the aggregate just
  because every other feeder happened to go stale or get removed.
  `options_market::set_settlement_price_from_oracle` cross-calls a live
  price_oracle deployment to settle a series permissionlessly, as an
  alternative to the original `set_settlement_price`'s trusted-oracle-
  address flow.
- [`vault/`](vault) — a per-(token, tag) escrow ledger for any number of
  allowlisted tokens and integrator contracts,
  motivated by a gap discovered while testing options_market: that
  contract holds every writer's collateral and every buyer's premium in
  one undifferentiated balance, with no accounting of which balance is
  actually earmarked for which position. `deposit`/`withdraw` here are
  scoped to a namespaced `tag` (e.g. `(options_market, "series", id)`), so a withdrawal
  can never draw down more than was specifically deposited under that
  tag — regardless of what the vault's raw token balance happens to be
  from other tags' deposits. `options_market::escrow_series_to_vault` /
  `claim_refund_from_vault` now wire this in for the specific gap it was
  built for — see "Known gaps" below for the scope of that integration.
- [`timelock/`](timelock) — a delayed-execution admin (proposer /
  executor / canceller roles, predecessor dependencies) meant to hold the
  admin role on the other contracts so users always get an exit window.
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
  `price_oracle`, and `vault` each cross-call a live Multisig deployment
  as a permissionless alternative to their own admin-gated
  functions, via `is_executable(action_id, class)`, which adds a
  per-class timelock on top of `is_approved` (see "Timelock tiers"
  below). It can also act as a Soroban **custom account**
  (`__check_auth`) — set its address as a plain `admin` anywhere and
  authorize calls with M-of-N ed25519 signatures.
- [`common/`](common) — `zenith-common`, a plain rlib (no `#[contract]`,
  no exported spec entries) shared by options_market, price_oracle and
  vault: `require_admin_or_multisig(env, admin_key, Auth, class, err)`
  with `enum Auth { Admin, Multisig(multisig, action_id) }`, plus pause
  and admin-transfer helpers. Every admin function is a single
  `x_inner(env, Auth, ...)` called by both its admin-gated entrypoint
  and its `_via_multisig` twin, so no validation is duplicated between
  twins. Also hosts the storage-migration framework
  (`zenith_common::migrations`) and pinned-multisig upgrades
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

`options_market/src/test_resources.rs` measures CPU instructions,
memory bytes, and read/write ledger entries and bytes for its public
entrypoints (including the worst cases: `exercise_batch`,
`reclaim_batch` and `prune_positions` at `MAX_BATCH_SIZE`, and a full
`migrate_counters` page) and compares them with
`options_market/snapshots/resources/*.json`. Any metric more than 5%
above its snapshot fails the test (the tolerance can be set per
scenario). CI runs it against the **optimized wasm** (`--features
resource-wasm`), since native execution undercounts; each snapshot
records which mode it was measured in, and modes are never compared.
`scripts/resource-diff/resource_diff.py` posts the table to the job
summary.

```sh
cd options_market
cargo build --target wasm32-unknown-unknown --release
cargo test --features resource-wasm resource_snapshots                       # compare
UPDATE_SNAPSHOTS=1 cargo test --features resource-wasm resource_snapshots    # re-record
```

The measuring and comparing lives in `tools/resource-snapshot`, so
other crates can add the same `test_resources` module.

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
| `price_oracle::math::median` | bounded by the min and max; permutation invariant; equal to the value for a constant input; at least half the reports on each side; monotonic in each report; shifting every report by c shifts the median by c; one outlier can't move it outside the other reports' range; on the whole `i128` domain, returns the correct value or fails exactly where averaging the middle pair overflows |
| `price_oracle::outlier_filter::OutlierFilter::compute_robust_price` (MAD) | `None` for no reports; result bounded by the min and max; MAD ≥ 0; permutation invariant; for a constant input, returns that price with MAD 0; keeps a strict majority for k ≥ 1; monotonic in k; shifting by c shifts the price by c and leaves MAD and the kept set unchanged; never panics on the whole `i128` domain |
| `streams::math::mul_div_floor` | exact floor; operands commute; monotonic in the numerator and non-increasing in the divisor; `b <= c ⇒ result <= a`; fails exactly on overflow or a zero divisor, as documented |

The "fails exactly on overflow" properties pin down today's
panic-on-overflow behaviour of `calc_fee`, `calc_payout`, and `median`.
Once those functions return typed errors, these properties should be
tightened to `Ok(correct)` or `Err(overflow)` with no panic. TWAP and
Black-Scholes helpers don't exist in this repo yet, so their suites
should come with them. Kani proofs are tracked separately.

## Deploying

```sh
soroban contract deploy \
  --wasm target/wasm32-unknown-unknown/release/

Every event any of these four contracts publishes has a test that
decodes its actual payload via `TryFromVal` (topics and data), not
just a test that confirms an event fired — the intent being that
anything an off-chain indexer would need to parse out of an event is
pinned down by a test, so a change to a tuple's field order or type
shows up as a test failure rather than as a silently broken indexer.

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
| `upgrade_via_multisig(action_id, new_wasm_hash)` | price_oracle, vault: authorized only by the multisig pinned with `set_upgrade_multisig`, and only for `action_id == upgrade_action_id(new_wasm_hash)`, so approvals bind to one wasm on one contract. `Critical` class. |
| multisig `approve_upgrade` / `revoke_upgrade` / `upgrade` | The multisig upgrades itself only after **every** signer approves the exact hash, plus a 3-day `UPGRADE_DELAY`. Its signer set is immutable by design, and an upgrade could change it. |

While the stored version lags the code (right after an upgrade) or a
paginated migration is in flight, state-changing entrypoints fail with
`MigrationInProgress` (102). Views keep working. When upgrading through
the timelock, schedule `upgrade` and `migrate` as two calls in **one**
operation so the pair is atomic. Migration errors use the same codes in
every contract: 100 `SchemaVersionMismatch`, 101 `InvalidMigrationTarget`,
102 `MigrationInProgress`, 103 `NoMigrationInProgress`,
104 `InvalidMigrationBatch`.

## `options_market` reference

All amounts are fixed-point at `PRICE_PRECISION` (1e7) unless noted —
e.g. `10_000_000` means "1.0 contracts" or "1.0 units of the underlying."
Rates (implied vol, collateral ratio, fee bps) use their own precision as
documented per field.

### Admin

| Function | Description |
|---|---|
| `initialize(admin, oracle, collateral_token, fee_recipient)` | One-time setup. Panics with `AlreadyInitialized` if called twice. |
| `transfer_admin(new_admin)` | Hands off control. Requires the **current** admin's signature. |
| `transfer_admin_via_multisig(multisig_contract, action_id, new_admin)` | Permissionless alternative to `transfer_admin`: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the current admin's own signature. |
| `set_fee_rate(new_bps)` | Sets the protocol fee (basis points). Capped at `MAX_FEE_RATE_BPS` (1000 = 10%). |
| `set_fee_rate_via_multisig(multisig_contract, action_id, new_bps)` | Permissionless alternative to `set_fee_rate`: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the admin's own signature. Still enforces `MAX_FEE_RATE_BPS` — approval changes who can call it, not what rate is valid. |
| `pause()` / `unpause()` | Emergency stop. Blocks `create_series`, `update_premium`, `buy_option`, `write_option`. Does **not** block `exercise`, `set_settlement_price`, or `reclaim_collateral` — a pause winds existing positions down, it doesn't trap funds. |
| `pause_via_multisig(multisig_contract, action_id)` / `unpause_via_multisig(...)` | Permissionless alternative to `pause`/`unpause`: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the admin's own signature. No `require_auth()` — the M-of-N approval itself is what authorizes the call. |
| `upgrade(new_wasm_hash)` | Swaps the contract's executable via Soroban's deployer, keeping the same address, ID, and storage. |
| `upgrade_via_multisig(multisig_contract, action_id, new_wasm_hash)` | Permissionless alternative to `upgrade`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. Arguably the highest-value place for this pattern in the whole codebase — a contract's executable is the single most consequential thing about it. |
| `create_series(underlying, option_type, strike_price, expiry, premium, implied_vol)` | Lists a new series. `expiry` must be > 1 hour out; `premium` must be > 0 (`InvalidSeriesParams`). Capped at `max_active_series` (default `DEFAULT_MAX_ACTIVE_SERIES` = 50) **concurrently active** series per underlying symbol (`TooManySeriesForUnderlying`); see "Active-series cap" below. Each `(underlying, option_type, strike_price, expiry)` spec can be listed once until its series is pruned (a cancelled or settled series still owns it): a second listing fails with `DuplicateSeries`. Blocked with `MigrationPending` until `migrate_counters` finishes after an upgrade. |
| `create_series_via_multisig(multisig_contract, action_id, underlying, option_type, strike_price, expiry, premium, implied_vol)` | Permissionless alternative to `create_series`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. Same validation and active-series cap apply. |
| `update_premium(series_id, new_premium, new_implied_vol)` | Re-prices an Active series. `new_premium` must be > 0 (`InvalidSeriesParams`). |
| `update_premium_via_multisig(multisig_contract, action_id, series_id, new_premium, new_implied_vol)` | Permissionless alternative to `update_premium`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. |
| `cancel_series(series_id)` | Cancels an Active series. Position holders then call `claim_refund` individually — the admin doesn't push funds to everyone in one call, since that would scale badly against Soroban's per-call resource limits. |
| `set_max_active_series(cap)` / `set_max_active_series_via_multisig(multisig_contract, action_id, cap)` | Sets the per-underlying cap on concurrently active series, `1..=MAX_ACTIVE_SERIES_CEILING` (500), else `InvalidActiveSeriesCap`. A params registry value for `max_ser` overrides it on the next registry version change. Multisig class: Standard. |
| `cancel_series_via_multisig(multisig_contract, action_id, series_id)` | Permissionless alternative to `cancel_series`: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the admin's own signature. Cancelling disrupts every open position in a series, so gating it behind M-of-N is at least as warranted as pause. |

### Oracle

| Function | Description |
|---|---|
| `set_settlement_price(series_id, price)` | The trusted-oracle-address flow: whatever address was set as `oracle` at `initialize` asserts a price directly. Records it and flips the series to `Settled`. One-shot: an already Settled series fails with `AlreadySettled`, a Cancelled one with `SeriesNotActive`. Records the settlement time as the series' retention anchor. |
| `set_settlement_price_from_oracle(series_id, oracle_contract)` | The permissionless alternative: anyone can settle an expired series by pointing at a live `price_oracle` deployment and letting it supply the price via a cross-contract call. No signature required — the price is already backed by that contract's own feeder-authenticated aggregate. |

### Traders

| Function | Description |
|---|---|
| `buy_option(buyer, series_id, contracts, max_premium)` | Opens a Long position. `max_premium` is slippage protection. Premium (net of the protocol fee) funds the pool `write_option` pays writers from. |
| `write_option(writer, series_id, contracts, collateral_amount, min_premium)` | Opens a Short position. Collateral: notional value for calls, 110% of strike for puts. Premium is paid out of the pool buyers have funded — `InsufficientPremiumPool` if no buyer has paid in enough yet (a write can't be paid a "premium" out of its own just-deposited collateral). `min_premium` is slippage protection: `PremiumBelowMinimum` if the net premium the writer would receive (after the protocol fee) is below it, so an `update_premium` cut or fee increase ordered ahead of the write can't underpay the writer. Pass `0` to opt out. |
| `exercise(owner, position_id)` | Long-side payout after expiry, within the 24h settlement window, if in the money. The 24h settlement window is a soft deadline (longs remain claimable until the 90-day forfeiture window). |
| `exercise_batch(owner, position_ids)` | Same as `exercise`, for every id in `position_ids` in one call — for an owner with several long positions who'd otherwise need one transaction per position. All-or-nothing (any single id failing `exercise`'s own checks aborts the whole batch) and capped at `MAX_BATCH_SIZE` (25). Returns the summed payout. |
 24h settlement window, if in the money. The 24h settlement window is a soft deadline (longs remain claimable until the 90-day forfeiture window). |
| `exercise_batch(owner, position_ids)` | Same as `exercise`, for every id in `position_ids` in one call — for an owner with several long positions who'd otherwise need one transaction per position. All-or-nothing (any single id failing `exercise`'s own checks aborts the whole batch) and capped at `MAX_BATCH_SIZE` (25). Returns the summed payout. |
| `buy_batch(buyer, orders)` | `buy_option` for up to `MAX_BATCH_SIZE` (25) `(series_id, contracts, max_premium)` orders — e.g. a strangle or ladder — with ONE premium pull and ONE fee transfer for the whole batch. Same per-order validation as `buy_option` (shared internals), orders applied in the given order (duplicate series allowed), all-or-nothing, one `option_bought` event per order. Returns the new position ids. ~51% less CPU than 5 separate `buy_option` calls in the test harness. |
| `write_batch(writer, orders)` | `write_option` for up to 25 `(series_id, contracts, collateral_amount)` orders, with ONE collateral pull and ONE premium payout. Orders draw on the premium pool strictly in order; all-or-nothing; one `option_written` event per order. |
| `settle_long(position_id)` | Permissionless auto-exercise / keeper settlement for in-the-money longs after settlement. Anyone can call; payout is unconditionally transferred to the position owner. |
| `sweep_forfeited(position_id)` | Admin entrypoint to sweep unexercised ITM long payouts after the 90-day forfeiture deadline to the protocol treasury/fee recipient with a `payout_forfeited` event. |

| `reclaim_collateral(writer, position_id)` | Short-side payout after settlement: locked collateral minus the max loss paid out to longs. |
| `reclaim_batch(writer, position_ids)` | Batched `reclaim_collateral`, same all-or-nothing/`MAX_BATCH_SIZE` contract as `exercise_batch`. Returns the summed reclaim. |
| `transfer_position(from, to, position_id)` | Moves an open position to `to` (`from` must own it and sign); both parties' position lists are updated and `to` inherits exercise / reclaim / r
| `reclaim_collateral(writer, position_id)` | Short-side payout after settlement: locked collateral minus the max loss paid out to longs. |
| `reclaim_batch(writer, position_ids)` | Batched `reclaim_collateral`, same all-or-nothing/`MAX_BATCH_SIZE` contract as `exercise_batch`. Returns the summed reclaim. |
| `transfer_position(from, to, position_id)` | Moves an open position to `to` (`from` must own it and sign); both parties' position lists are updated and `to` inherits exercise / reclaim / refund rights. Shorts are transferable because their collateral is already locked here, so `to` only gains the leftover-collateral reclaim. Rejected for exercised/settled positions and while paused; `to == from` is a no-op. Emits `position_transferred(from, to; position_id)`. |
| `split_position(owner, position_id, split_contracts)` | Splits `split_contracts` (`0 < split < contracts`, else `InvalidSplitAmount`) off an open position into a new position with the same series, side and owner. `premium_paid`, `fee_paid` and `collateral_locked` are divided pro rata, rounded down for the new position with the remainder kept in the original, so every total (and `SeriesEscrow`, open interest) is unchanged. Blocked while paused and for exercised/settled positions. Returns the new position id. |
| `claim_refund(owner, position_id)` | On a Cancelled series: buyers get their premium back (net of the fee already sent to `fee_recipient`), writers get their full collateral back. Paid directly from options_market's own balance. |
| `claim_refund_from_vault(vault_contract, owner, position_id)` | Same eligibility checks and refund formula as `claim_refund`, but pays out of a deployed `vault`'s `series_id`-tagged escrow instead — see "Vault integration" below. Requires `escrow_series_to_vault` to have moved this series' liability into `vault` first. |

### Pruning and the active-series cap

| Function | Description |
|---|---|
| `prune_positions(position_ids)` | Permissionless. Removes up to `MAX_BATCH_SIZE` (25) terminal positions whose series' retention period has passed (see the retention policy below), plus each id's entry in its owner's `UserPositions` list, and emits `position_pruned(owner, series_id; OptionPosition)` with the full record. All-or-nothing: `NotPrunable` for a position that is not terminal or still carries a liability, `RetentionNotElapsed` before the retention period ends. Returns the number pruned. |
| `prune_series(series_id)` | Permissionless. Removes a terminal series once **all** its positions are pruned (`SeriesHasPositions` otherwise) and its retention period has passed, together with its `SeriesEscrow`, `SeriesIndex` (the spec can be listed again), `SeriesPositions`, `SeriesClosedAt` and `SeriesReleased` entries. Frees its active-series slot if it still held one, and emits `series_pruned(underlying, series_id; OptionSeries)`. |
| `release_series_slot(series_id)` | Permissionless. Frees the series' active-series slot if it no longer needs one (below). Returns whether it did; each series releases at most once. |
| `migrate_counters(limit)` | Permissionless, run after upgrading from a version without these counters. Reads at most `limit` (1..=`MAX_PAGE_SCAN` = 200) entries per call, first every position (to rebuild per-series counts), then every series (to rebuild `ActiveSeriesCount`). Returns `true` when done. Until then `create_series`, pruning and `release_series_slot` fail with `MigrationPending`. Fresh deployments start migrated. |

#### Retention policy

A **series is terminal** once it is `Settled` or `Cancelled`. Its
retention period, `PRUNE_RETENTION` (30 days), runs from its anchor:

- **Settled:** the later of the settlement time and the end of the exercise window (`expiry + settlement_window`).
- **Cancelled:** the cancellation time.

Series closed before this version have no recorded close time and fall
back to `expiry`, which is never earlier than the real anchor.

A **position is terminal**, and carries no liability, when:

| Series | Side | Terminal when |
|---|---|---|
| Cancelled | either | its refund was claimed (`is_settled`) |
| Settled | Short | its collateral was reclaimed (`is_settled`) |
| Settled | Long | exercised, auto-exercised or forfeited (`is_exercised`), or it expired worthless (payout 0) |

An unexercised in-the-money long is never pruned while its payout is
still claimable: it becomes terminal only after `sweep_forfeited` pays
it out (after the 90-day forfeiture window). An unreclaimed short and an
unrefunded position of a cancelled series are never pruned either.

Pruned ids are never reused. `get_position` / `get_series` return
`None` for them; `get_position_status` / `get_series_status` return
`Pruned` for an allocated id whose entry was pruned, `None` for an id
that was never allocated, and `Live(record)` otherwise. The
`position_pruned` / `series_pruned` events are the permanent record for
indexers. Oracle history is out of scope (it has its own ring buffer).
There is no keeper bounty yet.

#### Active-series cap

`ActiveSeriesCount(underlying)` counts series that hold a listing slot.
`create_series` takes a slot; a series gives it back exactly once
(guarded by `SeriesReleased(series_id)`) when it:

- is **Settled** and its exercise window has closed. Unexercised ITM longs don't keep it active: they're tracked in `OrphanedLiabilities` and still block pruning until swept;
- is **Cancelled** and every position has taken its refund. An empty series is released as soon as it's cancelled, otherwise on the last refund;
- is **pruned**, if it hadn't released already.

An Active series, including one past expiry that hasn't been settled,
always counts. Cancelled releases happen automatically; a settled
series is released by anyone calling `release_series_slot` (or
`prune_series`) once its window has closed. There is no `Voided` state
in this contract; `Cancelled` covers it. `get_series_count_for_underlying`
still returns the lifetime count, but it no longer caps anything.

### Vault integration

| Function | Description |
|---|---|
| `escrow_series_to_vault(vault_contract, series_id)` | For a Cancelled series: moves its entire remaining refund liability — tracked incrementally in `SeriesEscrow` since the series' first `buy_option`/`write_option`, debited by whichever claim path (`claim_refund` or `claim_refund_from_vault`) any given position actually uses — out of options_market's own shared balance and into `vault`, tagged by `series_id`. Permissionless (only relocates the contract's own funds into a vault it already trusts, authorizes nothing new); callable at most once per series, since it zeroes `SeriesEscrow` on success. See "Known gaps" for exactly what this does and doesn't fix. |

### Views

`get_admin`, `is_paused`, `get_fee_rate`, `get_premium_pool`,
`get_series_count_for_underlying` (lifetime), `get_active_series_count`,
`get_max_active_series`, `get_series_position_counts` (`{ live, open }`),
`get_migration_state`, `is_migrated`, `get_position_status`,
`get_series_status`,
`get_series_id`, `get_series`, `get_position`,
`get_user_positions`, `get_underlying_price`, `get_series_escrow`
(remaining not-yet-claimed refund liability for a series), `get_stats`
(total premiums collected, total open interest, series count),
`get_orphaned_liabilities`, `sync_orphaned_liabilities`.

Paginated views (`limit` must be 1..=`MAX_PAGE_LIMIT` (50), else
`InvalidPageLimit`). Each call reads at most `MAX_PAGE_SCAN` (200)
entries, matching or not, so a page may come back short — keep paging
until `next_cursor` is 0. Missing (e.g. archived) entries are skipped.
At `limit = 50` in the test harness: `get_series_page` ≈ 2.0M CPU /
187KB, `get_user_positions_page` ≈ 1.7M CPU / 162KB.

| View | Description |
|---|---|
| `get_series_page(cursor, limit, filter)` | Series with id > `cursor`, in id order. `SeriesFilter { state, underlying, option_type }` — each a list of accepted values, empty = any. `state` is the stored state (stays `Active` past expiry until settled/cancelled). Returns `SeriesPage { items, next_cursor }`. |
| `get_user_positions_page(user, cursor, limit, side)` | `user`'s positions from index `cursor` of their position list, optionally only `Long` or `Short`. Returns `PositionPage { items, next_cursor }`. |
| `get_series_by_underlying(symbol)` | Every series id listed on `symbol`, oldest first (secondary index maintained on create; ids of pruned series are dropped). |
| `get_position_value(position_id)` | **Indicative only — never used for settlement.** Intrinsic value at the settlement price if set, else the last recorded underlying price (0 if neither). Positive for longs, negative for shorts, 0 once exercised/settled. |
| `get_account_summary(user)` | **Indicative only.** `AccountSummary { open_positions, long_value, short_liability, collateral_locked, net_value }` over `user`'s open positions. |

### Errors

| # | Error | | # | Error |
|---|---|---|---|---|
| 1 | `AlreadyInitialized` | | 18 | `SeriesNotCancelled` |
| 2 | `Unauthorized` | | 19 | `InvalidFeeRate` |
| 3 | `SeriesNotFound` | | 20 | `TooManySeriesForUnderlying` (active-series cap) |
| 4 | `SeriesNotActive` | | 21 | `InsufficientPremiumPool` |
| 5 | `SeriesNotExpired` | | 22 | `InvalidSeriesParams` |
| 6 | `PositionNotFound` | | 23 | `InvalidBatchSize` |
| 7 | `InsufficientPremium` | | 24 | `NothingToEscrow` |
| 8 | `InsufficientCollateral` | | 25 | `PremiumBelowMinimum` |
| 9 | `AlreadyExercised` | | 26 | `NotEligibleForForfeiture` |
| 10 | `AlreadySettled` | | 27 | `DuplicateSeries` |
| 11 | `ExerciseWindowClosed` | | 28 | `InvalidSplitAmount` |
| 12 | `ZeroContracts` | | 29 | `NotPrunable` |
| 13 | `PriceNotSet` | | 30 | `RetentionNotElapsed` |
| 14 | `NotInTheMoney` | | 31 | `SeriesHasPositions` |
| 15 | `WrongSide` | | 32 | `MigrationPending` |
| 16 | `ExpiryTooSoon` | | 33 | `InvalidActiveSeriesCap` |
| 17 | `ContractPaused` | | 34 | `InvalidPageLimit` |

## `price_oracle` reference

### Admin

| Function | Description |
|---|---|
| `initialize(admin)` | One-time setup. Defaults `max_staleness` to 1 hour and `min_reports` to 1. |
| `transfer_admin(new_admin)` | Hands off control. Requires the **current** admin's signature. |
| `transfer_admin_via_multisig(multisig_contract, action_id, new_admin)` | Permissionless alternative to `transfer_admin`: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the current admin's own signature. |
| `add_feeder(feeder)` / `remove_feeder(feeder)` | Authorize/revoke a price reporter. Capped at `MAX_FEEDERS` (16). A removed feeder's past reports stay readable via `get_latest_report` (audit trail) but no longer count toward the aggregate. |
| `add_feeder_via_multisig(multisig_contract, action_id, feeder)` / `remove_feeder_via_multisig(...)` | Permissionless alternatives: cross-call a deployed `multisig` and check `is_executable(action_id, class)` instead of requiring the admin's own signature. Still enforce `FeederAlreadyAdded`/`TooManyFeeders`/`FeederNotFound` — approval changes who can call these, not the underlying invariants. |
| `set_max_staleness(seconds)` | How old a report can be and still count toward `get_price`. Rejects zero. |
| `set_max_staleness_via_multisig(multisig_contract, action_id, seconds)` | Permissionless alternative: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the admin's own signature. Still rejects zero — approval changes who can call it, not what a valid staleness bound is. |
| `set_min_reports(count)` | How many CURRENTLY-fresh feeder reports `get_price` requires before it returns an aggregate at all. Without this, a single fresh report is enough the moment every other feeder's report goes stale or gets removed — that one feeder then fully determines the price with no averaging effect. Rejects zero. |
| `set_min_reports_via_multisig(multisig_contract, action_id, count)` | Permissionless alternative: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the admin's own signature. Still rejects zero. |
| `pause()` / `unpause()` | Emergency stop. Blocks `add_feeder`, `remove_feeder`, `report_price`. Does **not** block `get_price` — a pause freezes changes to the feed, it doesn't hide the last-known price. |
| `pause_via_multisig(multisig_contract, action_id)` / `unpause_via_multisig(...)` | Permissionless alternative to `pause`/`unpause`: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the admin's own signature. No `require_auth()` — the M-of-N approval itself is what authorizes the call. |

### Feeders

| Function | Description |
|---|---|
| `report_price(feeder, symbol, price)` | Records this feeder's own observation. Requires the feeder's signature and current authorization; rejects non-positive prices. |

### Views

| Function | Description |
|---|---|
| `get_price(symbol)` | Median across every currently-authorized feeder's report that's within `max_staleness` — but only if at least `min_reports` of them are fresh; otherwise `None` even if some reports exist. `None` if nothing is fresh — callers must not treat that as "price is zero." |
| `get_latest_report(symbol, feeder)` | One feeder's raw report, regardless of freshness or current authorization. |
| `is_feeder`, `get_feeder_count`, `get_max_staleness`, `get_min_reports`, `get_admin`, `is_paused` | |

### Events

`admin_transferred`, `paused`, `unpaused`, `max_staleness_updated`,
`min_reports_updated`, `feeder_added`, `feeder_removed`,
`price_reported` — one per state change, so an off-chain indexer doesn't
have to poll every view function to track what changed.

### Errors

| # | Error |
|---|---|
| 1 | `AlreadyInitialized` |
| 2 | `FeederAlreadyAdded` |
| 3 | `FeederNotFound` |
| 4 | `NotAFeeder` |
| 5 | `InvalidPrice` |
| 6 | `ContractPaused` |
| 7 | `InvalidStaleness` |
| 8 | `TooManyFeeders` |
| 9 | `Unauthorized` |
| 10 | `InvalidMinReports` |

## `vault` reference

A per-(token, tag) escrow ledger. One deployment holds any number of
allowlisted tokens and serves any number of integrator contracts
(`options_market`, an LP pool, an insurance fund, ...). The token passed
to `initialize` is the **default token** used by the `_legacy` wrappers.

### Tag scheme

A tag is a `#[contracttype] struct Tag { owner: Address, kind: Symbol, id: u64 }`,
and the storage key is the full triple, so two integrators — or two id
spaces inside one integrator — can never collide by construction.
`options_market` uses `(self, "series", series_id)` for series escrow and
reserves `(self, "position", position_id)` for per-position custody.

A plain struct was chosen over a `BytesN<32>` hash of it: the key is only
an address + a short symbol + a u64 larger than a hash, and in exchange
storage keys and events stay self-describing (an indexer doesn't need a
preimage registry to tell what a tag refers to).

Legacy `u64` tags map to `(legacy owner, "legacy", id)`, where the legacy
owner is the admin at `initialize` time (fixed, so a later
`transfer_admin` can't orphan legacy balances).

### Trust model

- **Tag owners** (integrators) are the only ones who can move escrow.
  A tag's owner is recorded as `TagOwner(tag) = tag.owner` on its first
  deposit, and `withdraw` / `transfer_tag` require that owner's
  `require_auth()`. In the intended integration the owner is a contract,
  so its own cross-contract call satisfies the check without a human
  signature. A bug in one integrator can therefore only ever reach its
  own tags.
- **Creating a tag** requires `tag.owner` to be in the integrator
  registry (or be the legacy owner) and to authorize. End users can
  deposit into a contract-owned tag only with that contract's auth, and
  the contract still owns it. `transfer_tag` between different owners
  requires **both** owners to authorize.
- **The admin** configures the vault (token allowlist, integrator
  registry, pause, sweeping untagged funds) but owns no integrator's
  tags. Moving escrow without the owner is an **emergency-only** power,
  available solely through multisig approval (`withdraw_via_multisig`,
  `transfer_tag_via_multisig`), and still capped at each tag's balance.

### Admin

| Function | Description |
|---|---|
| `initialize(admin, token, max_pause_duration)` | One-time setup. `token` becomes the default token and is allowlisted. `max_pause_duration` (seconds, non-zero) is immutable afterward — see "Pause and the escape hatch". |
| `transfer_admin(new_admin)` | Hands off control. Requires the **current** admin's signature. Does not hand over any tag. |
| `transfer_admin_via_multisig(multisig_contract, action_id, new_admin)` | Permissionless alternative to `transfer_admin`: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the current admin's own signature. |
| `pause()` / `unpause()` | Emergency stop. Blocks **both** `deposit` and `withdraw` (and `transfer_tag`, `set_beneficiary`). Time-bounded: see "Pause and the escape hatch" below. |
| `pause_via_multisig(multisig_contract, action_id)` / `unpause_via_multisig(...)` | Permissionless alternative to `pause`/`unpause`: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the admin's own signature. No `require_auth()` — the M-of-N approval itself is what authorizes the call. |
| `set_token_allowed(token, allowed)` / `set_token_allowed_via_multisig(multisig_contract, action_id, token, allowed)` | Manages the token allowlist. Removing a token only blocks **new** deposits; existing balances stay withdrawable, transferable and sweepable. |
| `set_integrator(integrator, allowed)` / `set_integrator_via_multisig(multisig_contract, action_id, integrator, allowed)` | Manages the integrator registry (who may create tags). Deregistering blocks new tags only. |
| `withdraw(token, tag, to, amount)` | Pays `amount` of `(token, tag)`'s escrow to `to`. Requires the tag owner. Panics with `InsufficientEscrowBalance` if the tag doesn't have that much earmarked, and `InsufficientVaultBalance` (instead of a token error) if the vault's actual balance can't cover it. Emits `shortfall_detected` if the ledger is under-backed. |
| `withdraw_via_multisig(multisig_contract, action_id, token, tag, to, amount)` | Permissionless alternative to `withdraw`: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the admin's own signature. Meant for manual recovery/migration when the calling contract itself can't produce that signature. Still enforces `InsufficientEscrowBalance`. |
| `sweep_untagged(token, to)` / `sweep_untagged_via_multisig(multisig_contract, action_id, token, to)` | Recovers `token` that landed on the vault directly, bypassing `deposit`: the actual balance minus `get_total_escrowed(token)`. Panics with `NoUntaggedFunds` if there's nothing to recover — including when there is a shortfall instead. |
| `transfer_tag(token, from_tag, to_tag, amount)` | Reassigns escrow between tags with no token movement — meant for the roll_position case. Requires `from_tag`'s owner, and `to_tag`'s owner too if different. `TotalEscrowed` is unaffected. |
| `transfer_tag_via_multisig(multisig_contract, action_id, token, from_tag, to_tag, amount)` | Permissionless alternative to `transfer_tag`: cross-calls a deployed `multisig` and checks `is_executable(action_id, class)` instead of requiring the admin's own signature. A new `to_tag` is recorded as owned by `to_tag.owner`. |
| `withdraw_batch(ops: Vec<(token, tag, to, amount)>)` | Batched `withdraw`: one admin auth check, up to `MAX_BATCH` (50) entries, every entry validated against in-flight balances (the same tag may repeat) before anything is written, one token transfer per distinct recipient, all-or-nothing. One `withdrawn` event per entry. ~4x cheaper than sequential calls (10 entries: 0.92M vs 3.80M CPU instructions, 175KB vs 844KB memory). |
| `transfer_tag_batch(ops: Vec<(token, from_tag, to_tag, amount)>)` | Batched `transfer_tag`, same validation and all-or-nothing semantics. One `tag_transferred` event per entry. |
| `migrate_legacy(ids)` | Moves pre-upgrade `Escrow(u64)` entries to `Escrow(default token, legacy_tag(id))`, owned by the current admin; the old `TotalEscrowed` moves to the default token's total on the first call. Idempotent. |

### Depositors

| Function | Description |
|---|---|
| `deposit(from, token, tag, amount) -> i128` | Pulls `amount` of an allowlisted `token` from `from` and credits `(token, tag)`. Requires `from`'s signature (and, on a tag's first deposit, its owner's). Credits — and returns — the **measured** balance delta, not `amount`: a fee-on-transfer token credits only what arrived. Costs two extra `balance` reads per deposit. |
| `set_beneficiary(tag, beneficiary)` | Registers who may `emergency_withdraw` the tag. Requires the tag owner's signature; rejected while paused, so a beneficiary can't be redirected once the escape-hatch clock is running. |
| `emergency_withdraw(tag, beneficiary)` | Escape hatch: once the vault has been paused for more than `max_pause_duration`, the tag's registered beneficiary (signing) receives the tag's whole balance. Emits `emergency_withdrawn`. |

### Legacy wrappers (default token, `u64` tags)

`deposit_legacy(from, id, amount)`, `withdraw_legacy(id, to, amount)`,
`transfer_tag_legacy(from_id, to_id, amount)`, `balance_of_legacy(id)`,
`legacy_tag(id)`.

### Pause and the escape hatch

A pause is an incident tool, not a custody seizure. `pause` records
`PausedAt`; once the vault has counted as paused for **more than**
`max_pause_duration` (fixed at `initialize`), each tag's registered
beneficiary can `emergency_withdraw` that tag's balance without the
admin. Pause time is **cumulative**: unpausing adds the elapsed pause to
an accrued total, and that total only resets after the vault has stayed
unpaused for a full `max_pause_duration` — so a brief unpause/re-pause
can't restart the clock. Tags without a registered beneficiary have no
hatch. For options_market's series-tagged escrow, the market contract is
the tag owner and would have to register beneficiaries itself (out of
scope here; possible follow-up with per-position tags).

### Views

`balance_of(token, tag)`, `get_total_escrowed(token)`,
`get_shortfall(token)` (`max(0, TotalEscrowed(token) − balance)`),
`get_tag_owner(tag)`, `is_token_allowed(token)`, `is_integrator(addr)`,
`get_admin`, `get_token`, `is_paused`,
`get_tag_count`, `get_tags(cursor, limit)`, `verify_ledger(cursor, limit) -> (partial_sum, next_cursor)`,
`get_beneficiary(tag)`, `get_max_pause_duration`,
`get_paused_at`, `get_pause_duration` (seconds counted toward the limit now).

`get_tags`/`verify_ledger` page through the ActiveTags index (every tag
with a nonzero balance; `limit` capped at 100). The index uses
swap-remove, so read all pages against one ledger snapshot.

### Ledger reconciliation

The vault's invariant is `sum(Escrow(tag)) == TotalEscrowed <= token.balance(vault)`.
Monitors can check it over RPC:

```sh
count=$(stellar contract invoke --id $VAULT -- get_tag_count)
cursor=0; sum=0
while [ "$cursor" -lt "$count" ]; do
  read partial cursor < <(stellar contract invoke --id $VAULT -- \
    verify_ledger --cursor $cursor --limit 100 | tr -d '[]",' )
  sum=$((sum + partial))
done
total=$(stellar contract invoke --id $VAULT -- get_total_escrowed | tr -d '"')
[ "$sum" = "$total" ] && echo "ledger OK ($sum)" || echo "MISMATCH: $sum != $total"
```

Building with `--features invariants` compiles a full post-condition check
into every mutating entrypoint (tests/fuzzing only, never production wasm);
CI runs the suite that way, including a 10,000-op random fuzz test.

### Clawback risk

A SAC issuer with clawback enabled can reduce the vault's balance
directly, leaving it below `TotalEscrowed`. The vault cannot prevent
this; it makes it visible instead: `get_shortfall(token)` reports the gap,
`withdraw` emits `shortfall_detected` whenever it sees one (at the cost
of one extra `balance` read), payouts the vault can no longer cover fail
with `InsufficientVaultBalance`, and `sweep_untagged` returns
`NoUntaggedFunds` rather than trapping. Shortfall socialization across
tags is out of scope — the last withdrawers of an under-backed token
absorb it.

### Events

`admin_transferred`, `paused`, `unpaused`, `token_allowed`,
`integrator_set`, `legacy_migrated`, `beneficiary_set`,
`emergency_withdrawn` (topics `(emergency_withdrawn, beneficiary, tag)`,
data `amount`), and the token-scoped ones:
`deposited` / `withdrawn` (topics `token`, full `tag`; data `(from|to, amount)`),
`tag_transferred` (topic `token`; data `(from_tag, to_tag, amount)`),
`swept_untagged` (topics `token`, `to`), `shortfall_detected` (topic `token`).

### Errors

| # | Error |
|---|---|
| 1 | `AlreadyInitialized` |
| 2 | `InvalidAmount` |
| 3 | `InsufficientEscrowBalance` |
| 4 | `ContractPaused` |
| 5 | `NoUntaggedFunds` |
| 6 | `Unauthorized` |
| 7 | `TokenNotAllowed` |
| 8 | `UnregisteredIntegrator` |
| 9 | `InsufficientVaultBalance` |

## `multisig` reference

Every signer has a weight and `threshold` is a total weight: an action
is approved once the summed weight of its fresh approvals reaches it.
The legacy `initialize` gives every signer weight 1, which is plain
M-of-N. `approval_ttl` is fixed at initialization.

> **Note:** the timelock tiers (`is_executable`, `delays`) and custom
> account (`__check_auth`) documented below were described by #150, but
> their source was never committed. The multisig here was restored from
> #149 and does not implement them yet; `zenith-common`'s
> `is_executable` cross-call has nothing to call until they're rebuilt.

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

Meant to hold the admin role on every other Zenith contract, so any
parameter or code change is visible on-chain for at least `min_delay`
seconds before it can run. Modelled on OpenZeppelin's TimelockController.

| Function | Description |
|---|---|
| `initialize(min_delay, proposers, executors, cancellers)` | One-time setup. Empty `executors` means anyone may execute a ready operation. There is no admin afterwards. |
| `schedule(proposer, calls, predecessor, salt, delay) -> id` | Proposer-only. `calls` is a `Vec<Call { target, function, args }>`; `delay >= min_delay`. The id is `sha256(xdr((calls, predecessor, salt)))` (see `hash_operation`). Rejects an id that's already pending or done. |
| `execute(executor, calls, predecessor, salt)` | Executor-only (unless open). Requires the operation ready and `predecessor` (if any) done. Runs every call in order via `invoke_contract`; any revert reverts the whole batch. |
| `cancel(canceller, id)` | Canceller-only. Removes a pending operation. |

**Self-administration.** Soroban forbids re-entry, so a `Call` whose
`target` is the timelock itself is dispatched internally by `execute`:
`update_delay(u64)`, `grant_role(Role, Address)`, `revoke_role(Role, Address)`,
`set_open_executor(bool)`. These changes are therefore only reachable
through a delayed operation.

**Views:** `get_timestamp(id)` (0 unknown, 1 done, else ready-at),
`is_operation`, `is_operation_pending`, `is_operation_ready`,
`is_operation_done`, `get_min_delay`, `has_role(role, account)`,
`is_open_executor`, `hash_operation`.

**Events:** `scheduled`, `executed`, `cancelled`, `min_delay_changed`, `role_changed`.

**Errors:** 1 `AlreadyInitialized`, 2 `Unauthorized`, 3 `InsufficientDelay`,
4 `AlreadyScheduled`, 5 `NotReady`, 6 `PredecessorNotDone`, 7 `NotPending`,
8 `UnknownSelfCall`, 9 `EmptyOperation`.

### Operations guide

1. Deploy the timelock, then `transfer_admin(timelock)` on each contract.
2. A proposer schedules the change, e.g. `calls = [Call { target: options_market, function: "set_fee_rate", args: [25u32] }]`, with a unique `salt`, and announces the id.
3. During the delay users can exit; a canceller can `cancel(id)`.
4. Once `is_operation_ready(id)`, an executor calls `execute` with the exact same `calls`/`predecessor`/`salt`.
5. Use `predecessor` to force ordering (e.g. a migration after an `upgrade`).

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
| `max_ser` | options_market cap on concurrently active series per underlying | series | `[1, 500]` | local `MaxActiveSeries` (50) |

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

- **Cross-position accounting on cancellation is now opt-in via `vault`,
  not the default.** `claim_refund` still pays every position directly
  out of options_market's own undifferentiated balance, unchanged — a
  writer's collateral refund and a buyer's premium refund on the same
  cancelled series still nominally draw from that same shared pot.
  `escrow_series_to_vault` / `claim_refund_from_vault` are the fix, but
  they're additive alternatives a caller has to choose to use per
  series, not a replacement for `claim_refund`: the original 100+
  existing tests around `claim_refund` needed zero changes, and nothing
  requires a cancelled series to ever call `escrow_series_to_vault` at
  all. The integration tags escrow by `series_id` (not per-position) and
  tracks each series' outstanding liability incrementally in
  `SeriesEscrow`, debited by whichever claim path (`claim_refund` or
  `claim_refund_from_vault`) a given position actually uses — so calling
  `escrow_series_to_vault` after some positions already claimed the
  original way still only moves what's genuinely left, never double.
  It does NOT fix the deeper, separate issue below it (pooled premium):
  if `write_option` has already paid a buyer's premium contribution out
  to some writer as that writer's own premium before the series gets
  cancelled, `SeriesEscrow` still promises that buyer a full refund
  (matching `claim_refund`'s own existing behavior) even though part of
  what they contributed has already left the contract for good —
  `escrow_series_to_vault` would then try to move more into `vault` than
  the series actually has left, and fail on the token transfer rather
  than silently under- or over-paying anyone. Full routing of active
  trading (`buy_option`/`write_option`/`exercise`/`reclaim_collateral`)
  through `vault` — the only way to close that deeper gap — remains the
  real, invasive rewrite described below; this integration deliberately
  stops short of it.
- **No order matching.** This is a pooled market, not an order book —
  writers and buyers don't get matched 1:1, and the premium pool
  (`PremiumPool`) is shared across EVERY series in the contract, not
  scoped to one — so `write_option`'s `InsufficientPremiumPool` check is
  a contract-wide pool-level constraint, not a guarantee that any
  specific buyer's trade funded any specific writer's premium, or even
  that the writer and buyer are in the same series at all. That's a
  design choice, not a bug, but it's also the root cause of the
  cancellation edge case called out above: premium a buyer contributed
  can already be gone (paid to some writer, possibly in a different
  series) by the time their own series gets cancelled.
- **`set_settlement_price_from_oracle` is additive, not a replacement.**
  The original trusted-oracle-address flow (`set_settlement_price`)
  still exists unchanged — a series can be settled either way, and
  nothing stops mixing both across different series. That's intentional
  (zero risk to the original flow's existing test coverage) but means
  there's no enforcement that a series *must* use the cross-contract
  path just because a `price_oracle` deployment exists.
- **Every admin-gated function on options_market, price_oracle, and
  vault now has a `_via_multisig` alternative — except `initialize`.**
  options_market: `pause`, `unpause`, `transfer_admin`, `set_fee_rate`,
  `upgrade`, `cancel_series`, `create_series`, `update_premium`.
  price_oracle: `pause`, `unpause`, `transfer_admin`,
  `set_max_staleness`, `set_min_reports`, `add_feeder`, `remove_feeder`.
  vault: `pause`, `unpause`, `transfer_admin`, `set_token_allowed`,
  `set_integrator`, `sweep_untagged`, plus the emergency-only `withdraw`
  and `transfer_tag` (whose normal path is tag-owner-gated, not admin). `initialize` can't have one by construction — there
  is no admin, and therefore no Multisig deployment trusted by this
  contract, until it runs. Every `_via_multisig` function is additive
  (the original admin-gated version is unchanged) and checks
  `is_executable(action_id, class)` on a deployed Multisig instead of a
  single signature, sharing the same `x_inner` implementation (and so
  the same validation) as the original — approval
  only changes who can call a function, never what a valid call to it
  looks like. Callers still choose their own stable `action_id` scheme
  per function, since Multisig never interprets what an id means.
  `exercise_batch`/`reclaim_batch` (owner/writer-gated, same as the
  functions they batch) and `escrow_series_to_vault` (permissionless)
  fall outside this pattern entirely — none of them are admin-gated to
  begin with, so there's no single-signature check for a `_via_multisig`
  variant to replace.

## Handsoff notes

<!-- handsoff-issue-225 -->
- #225: Add CPU, Ledger-I/O, and TTL Resource Budget Tests for `price_oracle`, `vault`, and `timelock`
