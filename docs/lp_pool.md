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
| `initialize(admin, keeper, asset, market, max_utilization_bps)` | admin | One-time setup. |
| `set_keeper`, `set_max_utilization`, `set_series_cap(series_id, max_collateral)` | admin | Governance risk limits. A cap of 0 de-lists a series. |
| `set_deposit_caps(max_deposit_per_epoch, total_capacity)` | admin / `_via_multisig` | Caps on queued deposits per epoch and on `idle + locked + queued`; 0 = unlimited. |
| `set_fees(performance_fee_bps, management_fee_bps, treasury)` | admin / `_via_multisig` | `sum <= 10_000`; non-zero fees need a treasury. |
| `set_paused(paused)` | admin / `_via_multisig` | Blocks `deposit` only. |
| `deposit(user, amount)` | user | Queues `amount ≥ MIN_DEPOSIT` into the current epoch. |
| `request_withdraw(user, shares)` | user | Moves shares out of the balance into a withdrawal ticket for the current epoch. |
| `claim(user)` | anyone | Settles `user`'s tickets from processed epochs (shares credited, assets paid to `user`). `deposit`/`request_withdraw` claim first automatically. |
| `write_option(series_id, contracts, collateral, min_premium)` | keeper | Writes on `options_market` with the pool as writer. `collateral` must equal the market's required collateral: the pool authorizes a transfer of exactly that amount. |
| `reclaim(position_id)` | anyone | Pulls a settled position's collateral back from the market. |
| `set_expected_liabilities(amount)` | keeper | Mid-epoch mark-to-market (informational). |
| `process_epoch()` | keeper | Fixes the epoch price and converts every queued ticket. |

## Share price and mark-to-market

```
total_assets = idle + locked − expected_liabilities
share_price  = (total_assets + VIRTUAL_ASSETS) / (total_shares + VIRTUAL_SHARES)
```

- **idle** is the USDC the pool owns that isn't locked. It's tracked
  internally and never read from the token balance.
- **locked** is the collateral held in open short positions.
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
down, in the pool's favour.

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
- **Shares aren't a token yet.** Shares are internal balances. The
  SEP-41 `lp_token` contract exists for composability; wiring the pool
  to mint through it is a follow-up.

## Caps, fees and pause

- **Caps.** A deposit that would push the epoch's queued deposits over
  `max_deposit_per_epoch`, or `idle + locked + queued` over
  `total_capacity`, is rejected (`DepositCapExceeded` /
  `TotalCapacityExceeded`); it is never truncated to fit. Reaching a cap
  exactly is allowed and emits `cap_reached`. Lowering a cap below the
  current size never touches existing positions or queued tickets; it only
  blocks new deposits.
- **Fees.** At `process_epoch`, before conversions, the pool pays the
  treasury `performance_fee_bps` of the epoch's PnL (`idle` now minus `idle`
  after the previous close, so a losing epoch pays nothing) plus
  `management_fee_bps` of `idle`, both rounded down. Fees leave `idle`, so
  they never mint shares. With no treasury set, no fee is charged.
- **Pause.** `paused` rejects `deposit` with `Paused`. Withdrawal requests,
  `claim`, `process_epoch` and redemptions keep working.
- Admin calls accept the admin's signature or, via the `_via_multisig`
  twins, an executable multisig approval (`Standard` class).
