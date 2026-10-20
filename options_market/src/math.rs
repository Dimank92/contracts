//! Math helpers and constants for the options market contract.
//!
//! All monetary amounts use `PRICE_PRECISION` fixed-point scale (1e7):
//! `10_000_000` = "1.0 contracts" or "1.0 units".

/// Fixed-point denominator for prices and contract sizes.
/// 1 contract = 1.0 = 10_000_000.
pub const PRICE_PRECISION: i128 = 10_000_000;

/// Fixed-point denominator for rates (implied vol, collateral ratio).
/// 1.0 = 100% = 1_000_000_000.
pub const RATE_PRECISION: i128 = 1_000_000_000;

/// Default protocol fee in basis points (0.5%).
pub const DEFAULT_FEE_RATE_BPS: i128 = 50;

/// Maximum allowed protocol fee in basis points (10%).
pub const MAX_FEE_RATE_BPS: i128 = 1_000;

/// Maximum number of concurrently active series per underlying symbol.
pub const MAX_SERIES_PER_UNDERLYING: u32 = 50;

/// Minimum collateral ratio for put writers (110% of notional), in
/// RATE_PRECISION scale.  `1_100_000_000` = 110%.
pub const MIN_COLLATERAL_RATIO: i128 = 1_100_000_000;

/// Settlement window after expiry (24 hours in seconds).
pub const SETTLEMENT_WINDOW: u64 = 86_400;

/// Maximum positions per `exercise_batch` / `reclaim_batch` call.
pub const MAX_BATCH_SIZE: u32 = 25;

/// Maximum number of entries scanned per paginated operation (migrate_counters,
/// get_series_page, get_user_positions_page).
pub const MAX_PAGE_SCAN: u32 = 200;

/// How long after a series closes (settles or cancels) before its positions
/// and series record can be pruned: 30 days in seconds.
pub const PRUNE_RETENTION: u64 = 30 * 86_400;

// ─── fee calculation ────────────────────────────────────────────────────────

/// Calculates the protocol fee for a given `amount` at `fee_bps` basis
/// points.  Returns `floor(amount * fee_bps / 10_000)`.
///
/// Panics on overflow (amount × bps > i128::MAX), which only occurs for
/// astronomically large values unreachable in production.
pub fn calc_fee(amount: i128, fee_bps: i128) -> i128 {
    amount
        .checked_mul(fee_bps)
        .expect("fee overflow")
        .checked_div(10_000)
        .expect("fee div")
}

// ─── payout calculation ─────────────────────────────────────────────────────

/// Calculates the intrinsic-value payout for a position.
///
/// - **Call**: `max(0, settlement_price − strike_price) × contracts / PRICE_PRECISION`
/// - **Put**:  `max(0, strike_price − settlement_price) × contracts / PRICE_PRECISION`
///
/// Returns `floor(intrinsic × contracts / PRICE_PRECISION)`.
/// Returns 0 when out-of-the-money.
///
/// Panics on overflow.
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

    intrinsic
        .checked_mul(contracts)
        .expect("payout overflow")
        .checked_div(PRICE_PRECISION)
        .expect("payout div")
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_is_zero_for_zero_amount() {
        assert_eq!(calc_fee(0, 50), 0);
    }

    #[test]
    fn fee_rounds_down() {
        // 1 unit * 50 bps = 50 / 10_000 = 0.005 → floor = 0
        assert_eq!(calc_fee(1, 50), 0);
        // 200 units * 50 bps = 10_000 / 10_000 = 1
        assert_eq!(calc_fee(200, 50), 1);
    }

    #[test]
    fn call_payout_itm() {
        // strike 700, settlement 750, 1 contract => payout = 50 * 10_000_000 / 10_000_000 = 50
        let payout = calc_payout(true, 700_000_000, 750_000_000, PRICE_PRECISION);
        assert_eq!(payout, 50_000_000);
    }

    #[test]
    fn call_payout_otm_is_zero() {
        let payout = calc_payout(true, 700_000_000, 650_000_000, PRICE_PRECISION);
        assert_eq!(payout, 0);
    }

    #[test]
    fn put_payout_itm() {
        // strike 700, settlement 650, 1 contract => payout = 50
        let payout = calc_payout(false, 700_000_000, 650_000_000, PRICE_PRECISION);
        assert_eq!(payout, 50_000_000);
    }

    #[test]
    fn put_payout_otm_is_zero() {
        let payout = calc_payout(false, 700_000_000, 750_000_000, PRICE_PRECISION);
        assert_eq!(payout, 0);
    }
}
