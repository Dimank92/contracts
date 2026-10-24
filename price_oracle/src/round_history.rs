//! Per-symbol round history and the cumulative-price accumulator behind
//! `twap`. Rounds live in a `MAX_ROUNDS` ring buffer, so older rounds are
//! pruned by being overwritten; every read checks the stored `round_id`
//! and never returns a value from a pruned round.

use soroban_sdk::{Env, Symbol};

use crate::{
    ttl,
    types::{DataKey, RoundData, TwapData},
};

pub const MAX_ROUNDS: u64 = 256;
/// Longest TWAP window honoured; longer requests are clamped to it.
pub const MAX_WINDOW_LEDGERS: u32 = 30 * ttl::DAY_IN_LEDGERS;
/// Reports above this are rejected, so `price * elapsed_ledgers` always
/// fits in `u128` for any `u32` interval.
pub const MAX_PRICE: i128 = 1_000_000_000_000_000_000_000_000;

fn key(symbol: &Symbol, round_id: u64) -> DataKey {
    DataKey::Round(symbol.clone(), (round_id % MAX_ROUNDS) as u32)
}

pub fn latest(env: &Env, symbol: &Symbol) -> Option<RoundData> {
    let id: u64 = env
        .storage()
        .persistent()
        .get(&DataKey::LatestRound(symbol.clone()))?;
    get(env, symbol, id)
}

pub fn get(env: &Env, symbol: &Symbol, round_id: u64) -> Option<RoundData> {
    let round: RoundData = env.storage().persistent().get(&key(symbol, round_id))?;
    (round.round_id == round_id).then_some(round)
}

/// Records `price` as the symbol's newest round at the current ledger. A
/// second round in the same ledger replaces the first's price (the
/// accumulator is unchanged since no ledgers elapsed).
pub fn record(env: &Env, symbol: &Symbol, price: i128) -> RoundData {
    let now = env.ledger().sequence();
    let round = match latest(env, symbol) {
        None => RoundData {
            round_id: 1,
            price,
            ledger: now,
            cumulative: 0,
        },
        Some(prev) if prev.ledger == now => RoundData { price, ..prev },
        Some(prev) => RoundData {
            round_id: prev.round_id + 1,
            price,
            ledger: now,
            cumulative: cumulative_at(&prev, now),
        },
    };
    ttl::set_persistent(env, &key(symbol, round.round_id), &round);
    ttl::set_persistent(env, &DataKey::LatestRound(symbol.clone()), &round.round_id);
    round
}

/// Accumulator value at ledger `at >= round.ledger`, carrying the round's
/// price forward.
fn cumulative_at(round: &RoundData, at: u32) -> u128 {
    let elapsed = u128::from(at - round.ledger);
    // price <= MAX_PRICE and elapsed <= u32::MAX, so the product cannot
    // overflow; the sum wraps like the Uniswap accumulator, and only
    // differences over a bounded window are ever read.
    round
        .cumulative
        .wrapping_add((round.price as u128) * elapsed)
}

/// Time-weighted average price over the last `window_ledgers` (clamped to
/// `MAX_WINDOW_LEDGERS`). `None` if the window reaches before the oldest
/// retained round, or if the symbol has no rounds. When nothing has ever
/// been pruned the window starts at the first round instead.
pub fn twap(env: &Env, symbol: &Symbol, window_ledgers: u32) -> Option<TwapData> {
    if window_ledgers == 0 {
        return None;
    }
    let latest = latest(env, symbol)?;
    let now = env.ledger().sequence();
    let oldest = get(
        env,
        symbol,
        latest.round_id.saturating_sub(MAX_ROUNDS - 1).max(1),
    )?;

    let mut start = now.saturating_sub(window_ledgers.min(MAX_WINDOW_LEDGERS));
    if start < oldest.ledger {
        if oldest.round_id != 1 {
            return None;
        }
        start = oldest.ledger;
    }
    let span = now - start;
    let gap_ledgers = (now - latest.ledger).min(span);
    if span == 0 {
        return Some(TwapData {
            twap: latest.price,
            window_ledgers: 0,
            gap_ledgers: 0,
        });
    }

    // Greatest retained round with ledger <= start (oldest qualifies).
    let (mut lo, mut hi) = (oldest.round_id, latest.round_id);
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if get(env, symbol, mid)?.ledger <= start {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let base = get(env, symbol, lo)?;

    let sum = cumulative_at(&latest, now).wrapping_sub(cumulative_at(&base, start));
    Some(TwapData {
        twap: (sum / u128::from(span)) as i128,
        window_ledgers: span,
        gap_ledgers,
    })
}
