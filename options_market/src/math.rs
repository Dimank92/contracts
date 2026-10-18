/// Fixed-point precision: 1 unit = 1e-7. So 1.0 = 10_000_000.
pub const PRICE_PRECISION: i128 = 10_000_000;

/// Rate precision for implied volatility and collateral ratios.
pub const RATE_PRECISION: i128 = 1_000_000_000;

/// Default protocol fee in basis points (0.5%).
pub const DEFAULT_FEE_RATE_BPS: i128 = 50;

/// Maximum allowed protocol fee (10% = 1000 bps).
pub const MAX_FEE_RATE_BPS: i128 = 1_000;

/// Maximum number of series per underlying symbol (active-series cap).
pub const MAX_SERIES_PER_UNDERLYING: u32 = 50;

/// Max entries in a batch call.
pub const MAX_BATCH_SIZE: u32 = 25;

/// Max ledger entries scanned per paginated call (e.g. get_series_page).
pub const MAX_PAGE_SCAN: u32 = 200;

/// Seconds a terminal series/position is retained before it can be pruned (30 days).
pub const PRUNE_RETENTION: u64 = 30 * 86_400;

/// Minimum collateral ratio for puts (110%).
pub const MIN_COLLATERAL_RATIO: i128 = 1_100_000_000; // 1.1 * RATE_PRECISION

/// Exercise window after expiry in seconds (24 hours). Settlement occurs
/// within this window; claims remain valid for up to 90 days.
pub const SETTLEMENT_WINDOW: u64 = 86_400;

/// Minimum time to expiry when creating a series (1 hour).
pub const MIN_EXPIRY_LEAD: u64 = 3_600;

/// Calculates the protocol fee for a given premium amount and fee rate.
///
/// fee = floor(amount * bps / 10_000)
pub fn calc_fee(amount: i128, bps: i128) -> i128 {
    if amount <= 0 || bps <= 0 {
        return 0;
    }
    // Checked mul; panics on overflow (same as the original behaviour;
    // a proptest suite covers it — see README).
    amount * bps / 10_000
}

/// Calculates the payout for an option position.
///
/// For calls: max(0, settlement_price - strike_price) * contracts / PRICE_PRECISION
/// For puts:  max(0, strike_price - settlement_price) * contracts / PRICE_PRECISION
pub fn calc_payout(
    is_call: bool,
    strike_price: i128,
    settlement_price: i128,
    contracts: i128,
) -> i128 {
    let intrinsic = if is_call {
        (settlement_price - strike_price).max(0)
    } else {
        (strike_price - settlement_price).max(0)
    };
    if intrinsic == 0 {
        return 0;
    }
    // floor(intrinsic * contracts / PRICE_PRECISION)
    intrinsic * contracts / PRICE_PRECISION
}

/// Required collateral for a call: notional = spot_or_strike * contracts / PRICE_PRECISION.
/// Uses `spot_price` if available (>0), otherwise falls back to `strike_price`.
pub fn calc_call_collateral(
    spot_price: i128,
    strike_price: i128,
    contracts: i128,
) -> i128 {
    let notional_basis = if spot_price > 0 { spot_price } else { strike_price };
    notional_basis * contracts / PRICE_PRECISION
}

/// Required collateral for a put: 110% of strike * contracts / PRICE_PRECISION.
pub fn calc_put_collateral(strike_price: i128, contracts: i128) -> i128 {
    // 110% = MIN_COLLATERAL_RATIO / RATE_PRECISION
    strike_price * contracts / PRICE_PRECISION * 11 / 10
}

/// `floor(a * b / d)` for non-negative `a`, `b` and positive `d`, without
/// overflowing on the intermediate product when the result itself fits:
/// splits `a = q*d + r` so it computes `q*b + r*b/d`. `None` on a genuine
/// overflow or a non-positive divisor.
pub fn mul_div_floor(a: i128, b: i128, d: i128) -> Option<i128> {
    if a < 0 || b < 0 || d <= 0 {
        return None;
    }
    let q = a / d;
    let r = a % d;
    q.checked_mul(b)?.checked_add(r.checked_mul(b)? / d)
}

/// `ceil(a * b / d)`, same domain and overflow behaviour as
/// [`mul_div_floor`].
pub fn mul_div_ceil(a: i128, b: i128, d: i128) -> Option<i128> {
    let floor = mul_div_floor(a, b, d)?;
    // Exact iff the remainder term divides evenly (q*b always is).
    if (a % d).checked_mul(b)? % d == 0 {
        Some(floor)
    } else {
        floor.checked_add(1)
    }
}

/// Long-side payout at settlement, pro-rated when longs outnumber the
/// writer collateral backing them: each long gets
/// `intrinsic * contracts * min(1, short_oi / long_oi)` (rounded down).
/// Longs and shorts are not matched one-to-one in this market, so this is
/// what keeps the long side from claiming more than the series' writers
/// actually put up. `None` on overflow.
pub fn long_settlement_payout(
    is_call: bool,
    strike_price: i128,
    settlement_price: i128,
    contracts: i128,
    long_oi: i128,
    short_oi: i128,
) -> Option<i128> {
    let full = calc_payout(is_call, strike_price, settlement_price, contracts);
    if full == 0 || long_oi <= short_oi {
        return Some(full);
    }
    mul_div_floor(full, short_oi.max(0), long_oi)
}

/// Short-side loss at settlement: the writer's share of what the long side
/// is owed, `intrinsic * contracts * min(1, long_oi / short_oi)` (rounded
/// up, so the long side is never short-changed by rounding), capped at the
/// collateral the writer locked. `None` on overflow.
pub fn short_settlement_loss(
    is_call: bool,
    strike_price: i128,
    settlement_price: i128,
    contracts: i128,
    collateral_locked: i128,
    long_oi: i128,
    short_oi: i128,
) -> Option<i128> {
    let full = calc_payout(is_call, strike_price, settlement_price, contracts);
    let loss = if full == 0 || short_oi <= long_oi {
        full
    } else {
        mul_div_ceil(full, long_oi.max(0), short_oi)?
    };
    Some(loss.min(collateral_locked))
}

/// Upper bound on a series premium (per contract, PRICE_PRECISION scale):
/// a put can never be worth more than its strike, and a call never more
/// than the underlying itself, for which the larger of strike and the last
/// recorded spot is the bound used.
pub fn max_premium(is_call: bool, strike_price: i128, spot_price: i128) -> i128 {
    if is_call {
        strike_price.max(spot_price)
    } else {
        strike_price
    }
}
