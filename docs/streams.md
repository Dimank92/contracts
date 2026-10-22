# Streams

`streams/` pays out tokens linearly per second, like Sablier and Drips. It is
used for contributor pay and grants.

## Model

The funder deposits the full `total` when the stream is created. At time `t`:

```
streamed(t) = 0                                            if t < cliff
            = total                                        if t ≥ end
            = mul_div_floor(total, t − start, end − start) otherwise
```

The recipient can withdraw `streamed(now) − withdrawn` at any time. Every
step rounds down, so the total paid out can never exceed the deposit. The
`prop_withdrawn_never_exceeds_deposit` test checks this over random
schedules.

## API

| Function | Who | Notes |
|----------|-----|-------|
| `initialize(governance)` | once | `governance` can cancel any stream. |
| `create_stream(funder, recipient, token, total, start, cliff, end) -> id` | funder | Pulls `total` right away. Needs `start < end` and `start ≤ cliff ≤ end`. `start` may be in the past, in which case the elapsed share is available at once. |
| `withdraw(id, amount)` | recipient | `amount` can be at most the accrued balance. |
| `cancel(caller, id)` | funder or governance | Pays the recipient what has accrued and not been withdrawn (nothing before the cliff), and refunds the rest to the funder. The stream is then frozen. |
| `transfer(id, new_recipient)` | recipient | Unwithdrawn accruals move with the stream. |
| `set_funding_delegate(id, delegate)` | funder | Lets `delegate` (or nobody, with `None`) call the three funding entrypoints below. |
| `top_up(caller, id, amount)` | funder or delegate | Pulls `amount` from `caller` and extends `end` at the current rate: `end += amount × remaining_time ÷ remaining_total`, floored, so the rate can only round up. |
| `extend_duration(caller, id, extra_time)` | funder or delegate | Pushes `end` out by `extra_time` seconds with no new funds; the unvested remainder is re-spread (lower rate). |
| `increase_rate(caller, id, new_rate)` | funder or delegate | Raises the rate to `new_rate` tokens/second over the remaining time and pulls the difference from `caller`. `new_rate` must exceed the current rate. |
| `balance_of(id) -> (recipient, funder)` | view | What the recipient could withdraw now, and what the funder would get back if the stream were cancelled now. |
| `get_stream(id)`, `get_stream_count()` | view | |

## Funding operations

`top_up`, `extend_duration` and `increase_rate` rebase the schedule at a
checkpoint `(anchor, base)` kept in its own storage entry: whatever has
vested at `max(now, start)` is frozen into `base`, and only the unvested
remainder is re-spread between `anchor` and the new `end`. The recipient
is therefore never owed less than before the call, and withdrawals are
always computed from the schedule, never from token balances. They fail
with `StreamCanceled` on a cancelled stream and `StreamCompleted` once
`end` has passed or everything is withdrawn. Every pull (including
`create_stream`) checks the contract's balance grew by the full amount and
otherwise reverts with `TransferShortfall`, so fee-on-transfer or no-op
tokens can't leave a stream over-credited.

## Example

```bash
# 12-month grant of 120 000 USDC (7 decimals) with a 3-month cliff
stellar contract invoke --id $STREAMS --source treasury -- create_stream \
  --funder $TREASURY --recipient $DEV --token $USDC --total 1200000000000 \
  --start 1767225600 --cliff 1775001600 --end 1798761600
```
