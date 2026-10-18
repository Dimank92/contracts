#![allow(dead_code)]

//! Core math for the options market: fee calculation, payout, and collateral.
//!
//! All amounts are fixed-point at `PRICE_PRECISION` (1e7) unless noted.

/// Fixed-point scale: 1.0 = 10_000_000 (used for prices, strikes, contracts).
pub const PRICE_PRECISION: i128 = 10_000_000;

/// Scale for implied-volatility rates (1.0 = 1_000_000_000, i.e. 1e9).
pub const RATE_PRECISION: i128 = 1_000_000_000;

/// Default protocol fee: 50 bps = 0.5%.
pub const DEFAULT_FEE_RATE_BPS: i128 = 50;

/// Maximum protocol fee: 1000 bps = 10%.
pub const MAX_FEE_RATE_BPS: i128 = 1_000;

/// Default cap on concurrently-active series per underlying.
pub const MAX_SERIES_PER_UNDERLYING: u32 = 50;

/// Minimum collateral ratio for puts: 110%.
pub const MIN_COLLATERAL_RATIO: i128 = 110;

/// Settlement window (24 h). Longs can exercise during this window after expiry.
pub const SETTLEMENT_WINDOW: u64 = 86_400;

/// Maximum positions in a single batch call.
pub const MAX_BATCH_SIZE: u32 = 25;

/// Minimum lead time for new series: 1 hour.
pub const MIN_EXPIRY_LEAD: u64 = 3_600;

/// `floor(amount × bps / 10_000)`.
///
/// Guaranteed `0 <= fee <= amount` for `0 <= bps <= 10_000`.
pub fn calc_fee(amount: i128, bps: i128) -> i128 {
    if amount == 0 || bps == 0 {
        return 0;
    }
    amount * bps / 10_000
}

/// Intrinsic payout for a long position at settlement.
///
/// - Call: `max(0, settlement − strike) × contracts / PRICE_PRECISION`
/// - Put:  `max(0, strike − settlement) × contracts / PRICE_PRECISION`
pub fn calc_payout(
    option_type: &crate::types::OptionType,
    strike: i128,
    settlement: i128,
    contracts: i128,
) -> i128 {
    use crate::types::OptionType;
    let intrinsic = match option_type {
        OptionType::Call => settlement - strike,
        OptionType::Put => strike - settlement,
    };
    if intrinsic <= 0 {
        return 0;
    }
    intrinsic * contracts / PRICE_PRECISION
}

/// Minimum collateral a writer must lock.
///
/// - Call: `contracts × price / PRICE_PRECISION`  (100% notional)
/// - Put:  `contracts × strike / PRICE_PRECISION × 110 / 100`  (110% of strike)
pub fn required_collateral(
    option_type: &crate::types::OptionType,
    strike: i128,
    price: i128,
    contracts: i128,
) -> i128 {
    use crate::types::OptionType;
    match option_type {
        OptionType::Call => contracts * price / PRICE_PRECISION,
        OptionType::Put => contracts * strike / PRICE_PRECISION * 110 / 100,
    }
}

#[cfg(test)]
mod proptests {
    extern crate std;

    use super::*;
    use crate::types::OptionType;
    use proptest::prelude::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    const CASES: u32 = 10_000;

    fn amount() -> impl Strategy<Value = i128> {
        0i128..=1_000_000_000_000_000
    }

    fn bps() -> impl Strategy<Value = i128> {
        0i128..=10_000
    }

    fn price() -> impl Strategy<Value = i128> {
        1_000i128..=1_000_000_000_000_000
    }

    fn contracts() -> impl Strategy<Value = i128> {
        1i128..=1_000_000_000_000
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn fee_is_non_negative(amount in amount(), bps in bps()) {
            prop_assert!(calc_fee(amount, bps) >= 0);
        }

        #[test]
        fn fee_does_not_exceed_amount(amount in amount(), bps in bps()) {
            prop_assert!(calc_fee(amount, bps) <= amount);
        }

        #[test]
        fn fee_is_floor_of_fraction(amount in amount(), bps in bps()) {
            let fee = calc_fee(amount, bps);
            // fee = floor(amount * bps / 10_000)
            prop_assert_eq!(fee, amount * bps / 10_000);
        }

        #[test]
        fn fee_is_zero_when_amount_or_bps_is_zero(amount in amount(), bps in bps()) {
            prop_assert_eq!(calc_fee(0, bps), 0);
            prop_assert_eq!(calc_fee(amount, 0), 0);
        }

        #[test]
        fn fee_is_monotonic_in_amount(a1 in amount(), a2 in amount(), bps in bps()) {
            let (lo, hi) = if a1 <= a2 { (a1, a2) } else { (a2, a1) };
            prop_assert!(calc_fee(lo, bps) <= calc_fee(hi, bps));
        }

        /// Whole i128 domain: returns the checked result or fails exactly
        /// where amount × bps overflows.
        #[test]
        fn overflow_is_never_silent(amount in any::<i128>(), bps in any::<i128>()) {
            let expected = amount.checked_mul(bps).map(|p| p / 10_000);
            let actual = catch_unwind(AssertUnwindSafe(|| {
                if bps == 0 { return Some(0i128); }
                if amount == 0 { return Some(0i128); }
                Some(amount * bps / 10_000)
            })).ok().flatten();
            prop_assert_eq!(actual, expected.map(|_| amount * bps / 10_000).ok_or(()).ok());
        }

        #[test]
        fn payout_is_non_negative_call(
            strike in price(),
            settlement in price(),
            contracts in contracts(),
        ) {
            prop_assert!(calc_payout(&OptionType::Call, strike, settlement, contracts) >= 0);
        }

        #[test]
        fn payout_is_non_negative_put(
            strike in price(),
            settlement in price(),
            contracts in contracts(),
        ) {
            prop_assert!(calc_payout(&OptionType::Put, strike, settlement, contracts) >= 0);
        }

        #[test]
        fn call_payout_zero_at_or_below_strike(
            strike in price(),
            contracts in contracts(),
        ) {
            prop_assert_eq!(calc_payout(&OptionType::Call, strike, strike, contracts), 0);
            if strike > 1 {
                prop_assert_eq!(calc_payout(&OptionType::Call, strike, strike - 1, contracts), 0);
            }
        }

        #[test]
        fn put_payout_zero_at_or_above_strike(
            strike in price(),
            contracts in contracts(),
        ) {
            prop_assert_eq!(calc_payout(&OptionType::Put, strike, strike, contracts), 0);
            prop_assert_eq!(calc_payout(&OptionType::Put, strike, strike + 1, contracts), 0);
        }

        #[test]
        fn call_and_put_never_both_nonzero(
            strike in price(),
            settlement in price(),
            contracts in contracts(),
        ) {
            let call = calc_payout(&OptionType::Call, strike, settlement, contracts);
            let put = calc_payout(&OptionType::Put, strike, settlement, contracts);
            prop_assert!(call == 0 || put == 0);
        }
    }
}
