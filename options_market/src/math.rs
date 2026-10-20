#![no_std]

//! Pure arithmetic for the options market: fee calculation, payout at
//! settlement, and collateral requirements. No Soroban env needed.

use crate::rounding_policy::SafeMathRounding;
use crate::types::OptionType;

// ─── Precision constants ──────────────────────────────────────────────────────

/// Fixed-point scale for prices and contract sizes.
/// 1 contract = PRICE_PRECISION = 10^7 base units.
pub const PRICE_PRECISION: i128 = 10_000_000;

/// Fixed-point scale for rates (implied vol, collateral ratios).
/// 100% = 1_000_000_000.
pub const RATE_PRECISION: i128 = 1_000_000_000;

// ─── Fee constants ────────────────────────────────────────────────────────────

/// Default protocol fee: 50 bps = 0.5%.
pub const DEFAULT_FEE_RATE_BPS: i128 = 50;

/// Hard ceiling on the configurable fee rate: 1000 bps = 10%.
pub const MAX_FEE_RATE_BPS: i128 = 1_000;

// ─── Collateral constants ─────────────────────────────────────────────────────

/// Put writers must post 110% of the strike value (expressed as percent).
pub const MIN_COLLATERAL_RATIO: i128 = 110;

// ─── Settlement constants ─────────────────────────────────────────────────────

/// Exercise window: 24 hours after expiry (in seconds).
pub const SETTLEMENT_WINDOW: u64 = 86_400;

// ─── Batch / page constants ───────────────────────────────────────────────────

/// Maximum positions/series that batch entrypoints process in one call.
pub const MAX_BATCH_SIZE: u32 = 25;

/// Default (and initial) cap on concurrently active series per underlying.
pub const MAX_SERIES_PER_UNDERLYING: u32 = 50;

/// Absolute ceiling the admin can configure the active-series cap up to.
pub const MAX_ACTIVE_SERIES_CEILING: u32 = 500;

/// Maximum entries read per `get_series_page` / `migrate_counters` call.
pub const MAX_PAGE_SCAN: u32 = 200;

/// Maximum items returned per paginated view call.
pub const MAX_PAGE_LIMIT: u32 = 50;

// ─── Pruning constants ────────────────────────────────────────────────────────

/// Retention period after a series closes before its entries may be pruned:
/// 30 days in seconds.
pub const PRUNE_RETENTION: u64 = 30 * 86_400;

// ─── Protocol fee ─────────────────────────────────────────────────────────────

/// Calculate the protocol fee: `floor(amount × bps / 10_000)`.
///
/// Properties guaranteed:
/// - `0 ≤ calc_fee(amount, bps) ≤ amount` for `0 ≤ bps ≤ 10_000` and `amount ≥ 0`
/// - equals the true mathematical floor of `amount × bps / 10_000`
/// - zero when either `amount` or `bps` is zero
/// - monotone non-decreasing in `amount` (for fixed `bps ≥ 0`)
/// - monotone non-decreasing in `bps` (for fixed `amount ≥ 0`)
/// - panics exactly where `amount × bps` overflows `i128`
pub fn calc_fee(amount: i128, bps: i128) -> i128 {
    if amount == 0 || bps == 0 {
        return 0;
    }
    // floor(amount × bps / 10_000); panics if the product overflows.
    SafeMathRounding::mul_div_floor(amount, bps, 10_000)
        .expect("calc_fee: overflow in amount × bps")
}

// ─── Settlement payout ────────────────────────────────────────────────────────

/// Calculate the settlement payout for a long position:
/// `floor(contracts × max(0, intrinsic) / PRICE_PRECISION)`
///
/// where `intrinsic` is:
/// - call: `max(0, settlement_price − strike_price)`
/// - put:  `max(0, strike_price − settlement_price)`
///
/// Returns 0 when out of the money.
///
/// Properties guaranteed:
/// - non-negative
/// - equals `floor(contracts × intrinsic / PRICE_PRECISION)`
/// - zero at or out of the money
/// - call payout non-decreasing in `settlement_price`
/// - put payout non-increasing in `settlement_price`
/// - monotone non-decreasing in `contracts`
/// - `call(K, S) == put(S, K)` symmetry
/// - at most one of `call` or `put` side is non-zero for the same `(K, S)`
/// - panics exactly where `contracts × intrinsic` overflows `i128`
pub fn calc_payout(
    contracts: i128,
    strike_price: i128,
    settlement_price: i128,
    option_type: &OptionType,
) -> i128 {
    let intrinsic = match option_type {
        OptionType::Call => (settlement_price - strike_price).max(0),
        OptionType::Put => (strike_price - settlement_price).max(0),
    };
    if intrinsic == 0 || contracts == 0 {
        return 0;
    }
    SafeMathRounding::mul_div_floor(contracts, intrinsic, PRICE_PRECISION)
        .expect("calc_payout: overflow in contracts × intrinsic")
}

// ─── Required collateral ──────────────────────────────────────────────────────

/// Minimum collateral a writer must post for `contracts` at `strike_price`:
/// - call: `floor(strike_price × contracts / PRICE_PRECISION)` (100% notional)
/// - put:  `floor(strike_price × contracts × 110 / (100 × PRICE_PRECISION))` (110% of notional)
pub fn required_collateral(option_type: &OptionType, strike_price: i128, contracts: i128) -> i128 {
    match option_type {
        OptionType::Call => {
            // 100% of notional: floor(strike * contracts / PRICE_PRECISION)
            SafeMathRounding::mul_div_floor(strike_price, contracts, PRICE_PRECISION)
                .expect("required_collateral: overflow")
        }
        OptionType::Put => {
            // 110% of strike value: floor(strike * contracts * 110 / (100 * PRICE_PRECISION))
            // Computed as: floor(strike * contracts / PRICE_PRECISION) then * 110 / 100
            // to keep intermediate values manageable while preserving floor semantics.
            let notional = SafeMathRounding::mul_div_floor(strike_price, contracts, PRICE_PRECISION)
                .expect("required_collateral: overflow");
            SafeMathRounding::mul_div_ceil(notional, MIN_COLLATERAL_RATIO, 100)
                .expect("required_collateral: overflow")
        }
    }
}

// ─── Property tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod proptests {
    extern crate std;

    use super::*;
    use proptest::prelude::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    const CASES: u32 = 10_000;

    // ── Realistic domains (README: prices 1e3..1e15, contracts 1..1e12) ──────

    fn realistic_amount() -> impl Strategy<Value = i128> {
        (1i128..=1_000_000_000_000_000i128).boxed()
    }

    fn realistic_bps() -> impl Strategy<Value = i128> {
        (0i128..=10_000i128).boxed()
    }

    fn realistic_price() -> impl Strategy<Value = i128> {
        (1_000i128..=1_000_000_000_000_000i128).boxed()
    }

    fn realistic_contracts() -> impl Strategy<Value = i128> {
        (1i128..=1_000_000_000_000i128).boxed()
    }

    fn option_type_strategy() -> impl Strategy<Value = OptionType> {
        prop_oneof![Just(OptionType::Call), Just(OptionType::Put)]
    }

    // ── calc_fee properties ───────────────────────────────────────────────────

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        /// 0 ≤ fee ≤ amount for bps ≤ 10_000 and amount ≥ 0.
        #[test]
        fn fee_is_bounded_by_amount(
            amount in realistic_amount(),
            bps in realistic_bps(),
        ) {
            let fee = calc_fee(amount, bps);
            prop_assert!(fee >= 0);
            prop_assert!(fee <= amount);
        }

        /// fee = floor(amount × bps / 10_000).
        #[test]
        fn fee_equals_floor(
            amount in realistic_amount(),
            bps in realistic_bps(),
        ) {
            let fee = calc_fee(amount, bps);
            let prod = amount * bps;
            prop_assert!(fee * 10_000 <= prod);
            prop_assert!((fee + 1) * 10_000 > prod);
        }

        /// fee is monotone non-decreasing in amount.
        #[test]
        fn fee_monotone_in_amount(
            a1 in realistic_amount(),
            a2 in realistic_amount(),
            bps in realistic_bps(),
        ) {
            let (lo, hi) = if a1 <= a2 { (a1, a2) } else { (a2, a1) };
            prop_assert!(calc_fee(lo, bps) <= calc_fee(hi, bps));
        }

        /// fee is monotone non-decreasing in bps.
        #[test]
        fn fee_monotone_in_bps(
            amount in realistic_amount(),
            b1 in realistic_bps(),
            b2 in realistic_bps(),
        ) {
            let (lo, hi) = if b1 <= b2 { (b1, b2) } else { (b2, b1) };
            prop_assert!(calc_fee(amount, lo) <= calc_fee(amount, hi));
        }

        /// fee is zero when either argument is zero.
        #[test]
        fn fee_zero_when_either_arg_zero(amount in realistic_amount(), bps in realistic_bps()) {
            prop_assert_eq!(calc_fee(0, bps), 0);
            prop_assert_eq!(calc_fee(amount, 0), 0);
        }
    }

    /// Whole i128 domain: calc_fee returns the checked result or panics
    /// exactly where amount × bps overflows.
    #[test]
    fn fee_overflow_exactly_on_product_overflow() {
        let cases: &[(i128, i128)] = &[
            (i128::MAX, 2),           // product overflows
            (i128::MAX / 10_001, 10_001), // product overflows
            (i128::MAX / 10_000, 1),  // fits
            (0, i128::MAX),           // zero short-circuit
        ];
        for &(amount, bps) in cases {
            let overflows = amount.checked_mul(bps).is_none();
            let result = catch_unwind(AssertUnwindSafe(|| calc_fee(amount, bps)));
            if overflows {
                assert!(result.is_err(), "expected panic for amount={} bps={}", amount, bps);
            } else {
                assert!(result.is_ok(), "unexpected panic for amount={} bps={}", amount, bps);
            }
        }
    }

    // ── calc_payout properties ────────────────────────────────────────────────

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        /// payout ≥ 0.
        #[test]
        fn payout_non_negative(
            contracts in realistic_contracts(),
            strike in realistic_price(),
            settlement in realistic_price(),
            ot in option_type_strategy(),
        ) {
            let p = calc_payout(contracts, strike, settlement, &ot);
            prop_assert!(p >= 0);
        }

        /// payout = floor(contracts × intrinsic / PRICE_PRECISION).
        #[test]
        fn payout_equals_floor_of_intrinsic(
            contracts in realistic_contracts(),
            strike in realistic_price(),
            settlement in realistic_price(),
            ot in option_type_strategy(),
        ) {
            let intrinsic = match ot {
                OptionType::Call => (settlement - strike).max(0),
                OptionType::Put => (strike - settlement).max(0),
            };
            let payout = calc_payout(contracts, strike, settlement, &ot);
            if intrinsic == 0 {
                prop_assert_eq!(payout, 0);
            } else {
                let prod = contracts * intrinsic;
                prop_assert!(payout * PRICE_PRECISION <= prod);
                prop_assert!((payout + 1) * PRICE_PRECISION > prod);
            }
        }

        /// payout is zero at or out of the money.
        #[test]
        fn call_payout_zero_when_otm(
            contracts in realistic_contracts(),
            strike in realistic_price(),
        ) {
            // At the money: settlement == strike
            prop_assert_eq!(calc_payout(contracts, strike, strike, &OptionType::Call), 0);
            // OTM: settlement < strike
            if strike > 1 {
                prop_assert_eq!(calc_payout(contracts, strike, strike - 1, &OptionType::Call), 0);
            }
        }

        #[test]
        fn put_payout_zero_when_otm(
            contracts in realistic_contracts(),
            strike in realistic_price(),
        ) {
            // At the money: settlement == strike
            prop_assert_eq!(calc_payout(contracts, strike, strike, &OptionType::Put), 0);
            // OTM: settlement > strike
            prop_assert_eq!(calc_payout(contracts, strike, strike + 1, &OptionType::Put), 0);
        }

        /// Call payout is non-decreasing in settlement_price.
        #[test]
        fn call_payout_non_decreasing_in_settlement(
            contracts in realistic_contracts(),
            strike in realistic_price(),
            s1 in realistic_price(),
            s2 in realistic_price(),
        ) {
            let (lo, hi) = if s1 <= s2 { (s1, s2) } else { (s2, s1) };
            prop_assert!(
                calc_payout(contracts, strike, lo, &OptionType::Call)
                    <= calc_payout(contracts, strike, hi, &OptionType::Call)
            );
        }

        /// Put payout is non-increasing in settlement_price.
        #[test]
        fn put_payout_non_increasing_in_settlement(
            contracts in realistic_contracts(),
            strike in realistic_price(),
            s1 in realistic_price(),
            s2 in realistic_price(),
        ) {
            let (lo, hi) = if s1 <= s2 { (s1, s2) } else { (s2, s1) };
            prop_assert!(
                calc_payout(contracts, strike, lo, &OptionType::Put)
                    >= calc_payout(contracts, strike, hi, &OptionType::Put)
            );
        }

        /// Payout is monotone non-decreasing in contracts.
        #[test]
        fn payout_monotone_in_contracts(
            c1 in realistic_contracts(),
            c2 in realistic_contracts(),
            strike in realistic_price(),
            settlement in realistic_price(),
            ot in option_type_strategy(),
        ) {
            let (lo, hi) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };
            prop_assert!(
                calc_payout(lo, strike, settlement, &ot)
                    <= calc_payout(hi, strike, settlement, &ot)
            );
        }

        /// call(K, S) == put(S, K): symmetry.
        #[test]
        fn call_put_symmetry(
            contracts in realistic_contracts(),
            k in realistic_price(),
            s in realistic_price(),
        ) {
            let call_payout = calc_payout(contracts, k, s, &OptionType::Call);
            let put_payout = calc_payout(contracts, s, k, &OptionType::Put);
            prop_assert_eq!(call_payout, put_payout);
        }

        /// At most one of call/put is non-zero for the same (K, S).
        #[test]
        fn at_most_one_side_non_zero(
            contracts in realistic_contracts(),
            k in realistic_price(),
            s in realistic_price(),
        ) {
            let call_p = calc_payout(contracts, k, s, &OptionType::Call);
            let put_p = calc_payout(contracts, k, s, &OptionType::Put);
            prop_assert!(call_p == 0 || put_p == 0);
        }
    }
}
