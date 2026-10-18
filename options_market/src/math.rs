use crate::types::OptionType;

pub const PRICE_PRECISION: i128 = 10_000_000; // 1e7
pub const RATE_PRECISION: i128 = 1_000_000_000; // 1e9
pub const MIN_COLLATERAL_RATIO: i128 = 1_100_000_000; // 110% over-collateralization for puts
pub const SETTLEMENT_WINDOW: u64 = 86_400; // 24h soft window after expiry to exercise

pub const DEFAULT_FEE_RATE_BPS: i128 = 50; // 0.5%
pub const MAX_FEE_RATE_BPS: i128 = 1_000; // 10% hard ceiling, even for the admin

/// Lifetime cap on series listed per underlying, so storage scanned per
/// underlying stays bounded.
pub const MAX_SERIES_PER_UNDERLYING: u32 = 50;

/// Caps how many position_ids exercise_batch/reclaim_batch will process in
/// a single call, so a caller with a very large position count can't build
/// a batch that blows through Soroban's per-call resource limits.
pub const MAX_BATCH_SIZE: u32 = 25;

/// Protocol fee on an amount, given a rate in basis points (1 bps = 0.01%).
pub fn calc_fee(amount: i128, fee_rate_bps: i128) -> i128 {
    amount
        .checked_mul(fee_rate_bps)
        .unwrap()
        .checked_div(10_000)
        .unwrap()
}

/// Cash payout at settlement:
/// Call: max(0, settlement - strike) × contracts / PRICE_PRECISION
/// Put:  max(0, strike - settlement) × contracts / PRICE_PRECISION
pub fn calc_payout(
    option_type: &OptionType,
    strike: i128,
    settlement: i128,
    contracts: i128,
) -> i128 {
    let intrinsic = match option_type {
        OptionType::Call => (settlement - strike).max(0),
        OptionType::Put => (strike - settlement).max(0),
    };
    contracts
        .checked_mul(intrinsic)
        .unwrap()
        .checked_div(PRICE_PRECISION)
        .unwrap()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn calc_fee_computes_bps_of_amount() {
        assert_eq!(calc_fee(40_000_000, 50), 200_000); // 0.5% of 40M
        assert_eq!(calc_fee(40_000_000, 100), 400_000); // 1% of 40M
    }

    #[test]
    fn calc_fee_of_zero_bps_is_zero() {
        assert_eq!(calc_fee(40_000_000, 0), 0);
    }

    #[test]
    fn calc_fee_truncates_towards_zero_on_a_non_exact_division() {
        // 999 * 50 / 10_000 = 4.995, integer division truncates to 4 —
        // callers must not assume this rounds to the nearest bps.
        assert_eq!(calc_fee(999, 50), 4);
    }

    #[test]
    fn calc_payout_call_is_zero_at_the_money() {
        assert_eq!(
            calc_payout(&OptionType::Call, 700_000_000, 700_000_000, PRICE_PRECISION),
            0
        );
    }

    #[test]
    fn calc_payout_call_is_zero_out_of_the_money() {
        assert_eq!(
            calc_payout(&OptionType::Call, 700_000_000, 650_000_000, PRICE_PRECISION),
            0
        );
    }

    #[test]
    fn calc_payout_call_pays_the_intrinsic_value_in_the_money() {
        // Strike 700, settlement 750 -> 50 of intrinsic value, 1 contract.
        assert_eq!(
            calc_payout(&OptionType::Call, 700_000_000, 750_000_000, PRICE_PRECISION),
            50_000_000
        );
    }

    #[test]
    fn calc_payout_put_is_zero_out_of_the_money() {
        assert_eq!(
            calc_payout(&OptionType::Put, 700_000_000, 750_000_000, PRICE_PRECISION),
            0
        );
    }

    #[test]
    fn calc_payout_put_pays_the_intrinsic_value_in_the_money() {
        // Strike 700, settlement 650 -> 50 of intrinsic value, 1 contract.
        assert_eq!(
            calc_payout(&OptionType::Put, 700_000_000, 650_000_000, PRICE_PRECISION),
            50_000_000
        );
    }

    #[test]
    fn calc_payout_scales_linearly_with_contracts() {
        let one_contract =
            calc_payout(&OptionType::Call, 700_000_000, 750_000_000, PRICE_PRECISION);
        let three_contracts = calc_payout(
            &OptionType::Call,
            700_000_000,
            750_000_000,
            3 * PRICE_PRECISION,
        );
        assert_eq!(three_contracts, one_contract * 3);
    }
}

/// Property-based tests: algebraic properties over whole input domains
/// rather than hand-picked examples. See README "Property tests" for the
/// list of properties. Shrunk failures are persisted under
/// `proptest-regressions/` and must be committed alongside the fix.
#[cfg(test)]
mod proptests {
    extern crate std;

    use super::*;
    use proptest::prelude::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    /// Every property runs at least this many cases, in CI and locally.
    const CASES: u32 = 10_000;

    // Shared strategies for realistic domains.

    /// Prices from 1e3 to 1e15 (in PRICE_PRECISION units).
    fn price() -> impl Strategy<Value = i128> {
        1_000i128..=1_000_000_000_000_000
    }

    /// Contract sizes from 1 to 1e12.
    fn contracts() -> impl Strategy<Value = i128> {
        1i128..=1_000_000_000_000
    }

    /// Fee rates from 0 to 10,000 bps (0%..=100%), wider than
    /// MAX_FEE_RATE_BPS on purpose so the bound holds at the extreme.
    fn fee_bps() -> impl Strategy<Value = i128> {
        0i128..=10_000
    }

    /// Amounts large enough to exceed any realistic premium, small enough
    /// that `amount * 10_000` never overflows.
    fn amount() -> impl Strategy<Value = i128> {
        0i128..=i128::MAX / 10_000
    }

    fn option_type() -> impl Strategy<Value = OptionType> {
        prop_oneof![Just(OptionType::Call), Just(OptionType::Put)]
    }

    /// Intrinsic value, computed independently of calc_payout.
    fn intrinsic(option_type: &OptionType, strike: i128, settlement: i128) -> i128 {
        match option_type {
            OptionType::Call => (settlement - strike).max(0),
            OptionType::Put => (strike - settlement).max(0),
        }
    }

    /// Runs `f`, turning a panic into `None`.
    fn no_panic<T>(f: impl FnOnce() -> T) -> Option<T> {
        catch_unwind(AssertUnwindSafe(f)).ok()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        // ---- calc_fee ----

        #[test]
        fn calc_fee_is_bounded_by_zero_and_amount(a in amount(), bps in fee_bps()) {
            let fee = calc_fee(a, bps);
            prop_assert!(0 <= fee && fee <= a, "fee {} outside [0, {}]", fee, a);
        }

        #[test]
        fn calc_fee_is_the_truncated_exact_fee(a in amount(), bps in fee_bps()) {
            // fee = floor(a * bps / 10_000), i.e.
            // fee * 10_000 <= a * bps < (fee + 1) * 10_000.
            let fee = calc_fee(a, bps);
            let exact = a * bps;
            prop_assert!(fee * 10_000 <= exact);
            prop_assert!(exact < (fee + 1) * 10_000);
        }

        #[test]
        fn calc_fee_is_monotonic_in_amount(
            a1 in amount(),
            a2 in amount(),
            bps in fee_bps(),
        ) {
            let (lo, hi) = if a1 <= a2 { (a1, a2) } else { (a2, a1) };
            prop_assert!(calc_fee(lo, bps) <= calc_fee(hi, bps));
        }

        #[test]
        fn calc_fee_is_monotonic_in_rate(
            a in amount(),
            b1 in fee_bps(),
            b2 in fee_bps(),
        ) {
            let (lo, hi) = if b1 <= b2 { (b1, b2) } else { (b2, b1) };
            prop_assert!(calc_fee(a, lo) <= calc_fee(a, hi));
        }

        #[test]
        fn calc_fee_of_zero_is_zero(x in amount(), bps in fee_bps()) {
            prop_assert_eq!(calc_fee(0, bps), 0);
            prop_assert_eq!(calc_fee(x, 0), 0);
        }

        /// Whole i128 domain: either the correct value or a failure exactly
        /// where the checked arithmetic overflows. Once the typed-error work
        /// lands this becomes `Ok(correct)` / `Err(Overflow)` with no panic.
        #[test]
        fn calc_fee_overflow_is_never_silent(a in any::<i128>(), bps in any::<i128>()) {
            let expected = a.checked_mul(bps).map(|p| p / 10_000);
            let actual = no_panic(|| calc_fee(a, bps));
            prop_assert_eq!(actual, expected);
        }

        // ---- calc_payout ----

        #[test]
        fn calc_payout_is_non_negative(
            t in option_type(),
            strike in price(),
            settlement in price(),
            c in contracts(),
        ) {
            prop_assert!(calc_payout(&t, strike, settlement, c) >= 0);
        }

        #[test]
        fn calc_payout_is_the_truncated_exact_payout(
            t in option_type(),
            strike in price(),
            settlement in price(),
            c in contracts(),
        ) {
            let payout = calc_payout(&t, strike, settlement, c);
            let exact = c * intrinsic(&t, strike, settlement);
            prop_assert!(payout * PRICE_PRECISION <= exact);
            prop_assert!(exact < (payout + 1) * PRICE_PRECISION);
        }

        #[test]
        fn calc_payout_is_zero_at_or_out_of_the_money(
            t in option_type(),
            a in price(),
            b in price(),
            c in contracts(),
        ) {
            // Build the at/out-of-the-money pair directly instead of
            // filtering random pairs (which rejects about half of them).
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let (strike, settlement) = match t {
                OptionType::Call => (hi, lo),
                OptionType::Put => (lo, hi),
            };
            prop_assert_eq!(calc_payout(&t, strike, settlement, c), 0);
        }

        #[test]
        fn calc_payout_call_is_non_decreasing_in_settlement(
            strike in price(),
            s1 in price(),
            s2 in price(),
            c in contracts(),
        ) {
            let (lo, hi) = if s1 <= s2 { (s1, s2) } else { (s2, s1) };
            prop_assert!(
                calc_payout(&OptionType::Call, strike, lo, c)
                    <= calc_payout(&OptionType::Call, strike, hi, c)
            );
        }

        #[test]
        fn calc_payout_put_is_non_increasing_in_settlement(
            strike in price(),
            s1 in price(),
            s2 in price(),
            c in contracts(),
        ) {
            let (lo, hi) = if s1 <= s2 { (s1, s2) } else { (s2, s1) };
            prop_assert!(
                calc_payout(&OptionType::Put, strike, lo, c)
                    >= calc_payout(&OptionType::Put, strike, hi, c)
            );
        }

        #[test]
        fn calc_payout_is_monotonic_in_contracts(
            t in option_type(),
            strike in price(),
            settlement in price(),
            c1 in contracts(),
            c2 in contracts(),
        ) {
            let (lo, hi) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };
            prop_assert!(
                calc_payout(&t, strike, settlement, lo)
                    <= calc_payout(&t, strike, settlement, hi)
            );
        }

        /// Linear in contracts within rounding: splitting a position in two
        /// never pays out more than the whole, and loses at most 1 unit.
        #[test]
        fn calc_payout_is_additive_in_contracts_within_rounding(
            t in option_type(),
            strike in price(),
            settlement in price(),
            c1 in contracts(),
            c2 in contracts(),
        ) {
            let whole = calc_payout(&t, strike, settlement, c1 + c2);
            let parts = calc_payout(&t, strike, settlement, c1)
                + calc_payout(&t, strike, settlement, c2);
            prop_assert!(whole - parts == 0 || whole - parts == 1,
                "whole {} vs parts {}", whole, parts);
        }

        /// Scaling contracts by k scales the payout by k, within k - 1 of
        /// rounding.
        #[test]
        fn calc_payout_scales_with_contracts_within_rounding(
            t in option_type(),
            strike in price(),
            settlement in price(),
            c in 1i128..=1_000_000_000,
            k in 1i128..=1_000,
        ) {
            let scaled = calc_payout(&t, strike, settlement, c * k);
            let times_k = calc_payout(&t, strike, settlement, c) * k;
            prop_assert!(times_k <= scaled && scaled - times_k < k,
                "k={} scaled {} vs {}", k, scaled, times_k);
        }

        /// Call and put payouts are mirror images: exactly one side can be
        /// in the money, and swapping strike and settlement swaps them.
        #[test]
        fn calc_payout_call_put_symmetry(
            strike in price(),
            settlement in price(),
            c in contracts(),
        ) {
            let call = calc_payout(&OptionType::Call, strike, settlement, c);
            let put = calc_payout(&OptionType::Put, strike, settlement, c);
            prop_assert!(call == 0 || put == 0);
            prop_assert_eq!(call, calc_payout(&OptionType::Put, settlement, strike, c));
        }

        /// Whole i128 domain: either the correct value or a failure exactly
        /// where the checked arithmetic overflows (see calc_fee above).
        #[test]
        fn calc_payout_overflow_is_never_silent(
            t in option_type(),
            strike in any::<i128>(),
            settlement in any::<i128>(),
            c in any::<i128>(),
        ) {
            let diff = match t {
                OptionType::Call => settlement.checked_sub(strike),
                OptionType::Put => strike.checked_sub(settlement),
            };
            let expected = diff
                .map(|d| d.max(0))
                .and_then(|i| c.checked_mul(i))
                .map(|p| p / PRICE_PRECISION);
            let actual = no_panic(|| calc_payout(&t, strike, settlement, c));
            prop_assert_eq!(actual, expected);
        }
    }
}
