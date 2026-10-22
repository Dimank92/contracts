# LP Pool: strategy and risk

`lp_pool` lets depositors supply USDC to a pool that writes options on
`options_market` for them. The pool is the writer counterparty, and it
earns the premium on every option it writes.

## Lifecycle

```
epoch N open ──► deposits / withdraw requests queue into N
             ──► keeper: write_option(...) within risk limits
             ──► series settle ──► anyone: reclaim(position_id)
             ──► keeper: process_epoch()   (requires zero open positions)
epoch N+1 open ──► users: claim(user) → shares / assets from epoch N
```

| Function | Who | What |
|---|---|---|
| `initialize(admin, keeper, asset, market, lp_token, max_utilization_bps)` | admin | One-time setup. Pins the `lp_token` contract the pool mints/burns through. |
| `set_keeper`, `set_max_utilization`, `set_series_cap(series_id, max_collateral)` | admin | Governance risk limits. A cap of 0 de-lists a series. |
| `request_deposit(user, amount)` | user | Queues `amount ≥ MIN_DEPOSIT` into the current epoch's deposit queue. No shares are minted yet. |
| `request_withdraw(user, shares)` | user | Burns nothing yet: moves `shares` out of `user`'s balance into a withdrawal ticket for the current epoch. |
| `claim(user)` | anyone | Settles `user`'s tickets from finalized epochs: mints shares for deposits, pays assets for withdrawals. `request_deposit`/`request_withdraw` claim first automatically. |
| `write_option(series_id, contracts, collateral, min_premium)` | keeper | Writes on `options_market` with the pool as writer. `collateral` must equal the market's required collateral: the pool authorizes a transfer of exactly that amount. |
| `reclaim(position_id)` | anyone | Pulls a settled position's collateral back from the market. |
| `set_expected_liabilities(amount)` | keeper | Mid-epoch mark-to-market (informational). |
| `process_epoch()` | keeper | Finalizes the current epoch: fixes the share price and converts every queued ticket. Requires zero open positions. |
| `share_price()` | anyone | Read-only. Current share price (see below). |
| `pending_deposit(user)` | anyone | Read-only. `user`'s queued deposit amount in the current epoch. |
| `pending_withdraw(user)` | anyone | Read-only. `user`'s queued withdrawal shares in the current epoch. |
| `pending_epoch()` | anyone | Read-only. The epoch index that is currently open for requests. |

## Epoch pricing rule

Each epoch has one deposit queue and one withdrawal queue. Requests made
during epoch `N` are **only** priced when epoch `N` is finalized by
`process_epoch`. A request can never be filled at the price it was
observed at: the price used is the one fixed at finalization, after all
open positions in the epoch have settled and been reclaimed. Re-requesting
in the same epoch adds to the existing ticket rather than creating a new
one, and `claim` can be called at any time after finalization — including
several epochs later — and always settles at the price fixed for the
epoch the ticket was queued in.

## Share price and mark-to-market

```
total_assets = idle + locked − expected_liabilities
share_price  = (total_assets + VIRTUAL_ASSETS) / (total_shares + VIRTUAL_SHARES)
```

- **idle** is the USDC the pool owns that isn't locked. It's tracked
  internally and never read from the token balance of the pool address;
  donations to the pool address do not change `idle`.
- **locked** is the collateral held in open short positions. It is
  excluded from the withdrawable base: withdrawals can only be filled
  from assets that are not locked against a live series.
- **Accrued premium**: `options_market` pays the writer's premium up front
  in `write_option`, so it is already part of `idle` when the option is
  written.
- **expected_liabilities** is the keeper's mark of what open positions
  will pay buyers, e.g. oracle intrinsic value per position. It only
  affects the informational `total_assets`/`share_price` views, and it
  resets to zero when the last position is reclaimed.

Deposits and withdrawals never convert at a marked price.
`process_epoch` refuses to run while any position is open, so at
conversion time `locked = 0` and every liability is **realized**:
`reclaim` credited exactly what the market returned. A loss larger than
the premium therefore lowers `idle`, which lowers the share price, and
every holder in that epoch shares it pro rata. Both conversions round
down, in the pool's favour; any rounding dust on share burn stays in the
pool and accrues to remaining holders.

## Shares and `lp_token`

Shares are SEP-41 balances held in the pinned `lp_token` contract. The
pool is the **only** minter and burner:

- `process_epoch` mints shares to the pool's own account for the epoch's
  deposit queue, then `claim` transfers them to each depositor.
- `request_withdraw` moves shares from the user into the pool's escrow
  account inside `lp_token`; `process_epoch` burns them once the epoch's
  price is fixed and `claim` pays out the corresponding assets.

Because minting happens only at finalization, the share price cannot be
moved by a same-ledger deposit/withdraw round-trip.

## Collateral deployment and `vault` tagging

Pool capital is deployed through `options_market`'s writer flow, never by
transferring directly to the market. Every `write_option` call:

1. Checks the series whitelist/cap and utilization limits against `idle`.
2. Authorizes `options_market` to pull exactly `collateral` from the pool.
3. Records the resulting position with the `vault` tag reserved for the
   pool, so the collateral is accounted separately from individual
   writers' collateral. Pool funds are never commingled with individual
   writers' collateral, and `reclaim` returns collateral to the pool's
   `idle` only.

While a position is open its collateral counts as `locked` and cannot
back a withdrawal; a withdrawal queued in that epoch is only filled after
the series settles and `reclaim` has returned the collateral.

## Risk limits

- **Whitelist and per-series cap**: `write_option` requires
  `series_locked + collateral ≤ series_cap(series_id)`, and a series
  with no cap can't be written.
- **Utilization cap**: `locked_after ≤ (idle + locked) × max_utilization_bps / 10 000`.
  The remaining idle buffer is what's left if every open position in the
  epoch is a total loss.
- **Only idle assets**: collateral can't exceed `idle`, so queued deposits
  and assets reserved for processed withdrawals are never put at risk.
- **Bounded positions**: at most `MAX_POSITIONS` (50) open at once, which
  keeps `reclaim` and `process_epoch` cheap.

## Gaming and inflation resistance

- **Settlement timing**: deposits and withdrawal requests only convert
  at `process_epoch`, which can't run until every position has settled.
  Nobody can enter just before an out-of-the-money expiry, or exit just
  before an in-the-money one.
- **First-depositor inflation**: three layers of protection.
  1. Internal accounting: donating tokens to the pool doesn't change
     `idle`.
  2. A virtual offset of `VIRTUAL_SHARES = 1000` shares and
     `VIRTUAL_ASSETS = 1`, as in ERC-4626.
  3. `MIN_DEPOSIT = 1000` base units.

  See `first_depositor_inflation_attack_is_neutralized`.

## Known risks (out of scope)

- **No hedging.** The pool is a naked short-volatility position within
  its caps. A market-wide move can lose the whole utilized fraction in
  one epoch.
- **Keeper trust.** The keeper chooses which whitelisted series to write
  and when, and its `expected_liabilities` mark is unverified. The mark
  only affects the views, never conversions.
- **Market dependency.** Every locked asset depends on `options_market`
  returning collateral in `reclaim_collateral`. If one position can't
  settle, `process_epoch` is blocked, and with it all deposits and
  withdrawals.
- **No secondary market.** Shares are transferable SEP-41 balances via
  `lp_token`, but the pool does not operate a market for them.
