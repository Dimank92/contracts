pub struct SafeMathRounding;

impl SafeMathRounding {
    /// Safe directional mul_div calculation rounding towards floor.
    /// Favors protocol solvency when calculating user withdrawals or payouts.
    pub fn mul_div_floor(a: i128, b: i128, denominator: i128) -> Option<i128> {
        if denominator == 0 {
            return None;
        }
        let prod = a.checked_mul(b)?;
        // checked_div/checked_rem: i128::MIN / -1 overflows.
        let res = prod.checked_div(denominator)?;
        let rem = prod.checked_rem(denominator)?;
        if rem != 0 && ((prod < 0) ^ (denominator < 0)) {
            res.checked_sub(1)
        } else {
            Some(res)
        }
    }

    /// Safe directional mul_div calculation rounding towards ceiling.
    /// Favors protocol solvency when assessing fees, collateral liabilities, or margins.
    pub fn mul_div_ceil(a: i128, b: i128, denominator: i128) -> Option<i128> {
        if denominator == 0 {
            return None;
        }
        let prod = a.checked_mul(b)?;
        // checked_div/checked_rem: i128::MIN / -1 overflows.
        let res = prod.checked_div(denominator)?;
        let rem = prod.checked_rem(denominator)?;
        if rem != 0 && !((prod < 0) ^ (denominator < 0)) {
            res.checked_add(1)
        } else {
            Some(res)
        }
    }
}

/// Property-based tests; see README "Property tests".
#[cfg(test)]
mod proptests {
    extern crate std;

    use super::*;
    use proptest::prelude::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    const CASES: u32 = 10_000;

    /// i64-range operands: the product always fits in i128, and so does
    /// quotient * denominator, so exactness can be checked without
    /// overflow in the test itself.
    fn operand() -> impl Strategy<Value = i128> {
        any::<i64>().prop_map(i128::from)
    }

    fn nonzero_denominator() -> impl Strategy<Value = i128> {
        any::<i64>()
            .prop_filter("non-zero", |d| *d != 0)
            .prop_map(i128::from)
    }

    /// True when q == floor(p / d), checked without division.
    fn is_floor(q: i128, p: i128, d: i128) -> bool {
        if d > 0 {
            q * d <= p && p < (q + 1) * d
        } else {
            q * d >= p && p > (q + 1) * d
        }
    }

    /// True when q == ceil(p / d), checked without division.
    fn is_ceil(q: i128, p: i128, d: i128) -> bool {
        if d > 0 {
            (q - 1) * d < p && p <= q * d
        } else {
            (q - 1) * d > p && p >= q * d
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn floor_is_the_exact_floor(
            a in operand(),
            b in operand(),
            d in nonzero_denominator(),
        ) {
            let q = SafeMathRounding::mul_div_floor(a, b, d).unwrap();
            prop_assert!(is_floor(q, a * b, d), "floor({} * {} / {}) = {}", a, b, d, q);
        }

        #[test]
        fn ceil_is_the_exact_ceil(
            a in operand(),
            b in operand(),
            d in nonzero_denominator(),
        ) {
            let q = SafeMathRounding::mul_div_ceil(a, b, d).unwrap();
            prop_assert!(is_ceil(q, a * b, d), "ceil({} * {} / {}) = {}", a, b, d, q);
        }

        /// ceil - floor is 1 on an inexact division and 0 on an exact one.
        #[test]
        fn ceil_and_floor_differ_by_at_most_one(
            a in operand(),
            b in operand(),
            d in nonzero_denominator(),
        ) {
            let lo = SafeMathRounding::mul_div_floor(a, b, d).unwrap();
            let hi = SafeMathRounding::mul_div_ceil(a, b, d).unwrap();
            let exact = (a * b) % d == 0;
            prop_assert_eq!(hi - lo, if exact { 0 } else { 1 });
        }

        #[test]
        fn operands_commute(
            a in operand(),
            b in operand(),
            d in nonzero_denominator(),
        ) {
            prop_assert_eq!(
                SafeMathRounding::mul_div_floor(a, b, d),
                SafeMathRounding::mul_div_floor(b, a, d)
            );
            prop_assert_eq!(
                SafeMathRounding::mul_div_ceil(a, b, d),
                SafeMathRounding::mul_div_ceil(b, a, d)
            );
        }

        /// ceil(x) == -floor(-x).
        #[test]
        fn ceil_is_negated_floor_of_negation(
            a in operand(),
            b in operand(),
            d in nonzero_denominator(),
        ) {
            let ceil = SafeMathRounding::mul_div_ceil(a, b, d).unwrap();
            let floor_neg = SafeMathRounding::mul_div_floor(-a, b, d).unwrap();
            prop_assert_eq!(ceil, -floor_neg);
        }

        #[test]
        fn monotonic_in_numerator_for_positive_factor_and_denominator(
            a1 in operand(),
            a2 in operand(),
            b in 0i128..=i64::MAX as i128,
            d in 1i128..=i64::MAX as i128,
        ) {
            let (lo, hi) = if a1 <= a2 { (a1, a2) } else { (a2, a1) };
            prop_assert!(
                SafeMathRounding::mul_div_floor(lo, b, d).unwrap()
                    <= SafeMathRounding::mul_div_floor(hi, b, d).unwrap()
            );
            prop_assert!(
                SafeMathRounding::mul_div_ceil(lo, b, d).unwrap()
                    <= SafeMathRounding::mul_div_ceil(hi, b, d).unwrap()
            );
        }

        /// Linear: multiplying by d and dividing by d is the identity.
        #[test]
        fn dividing_by_a_factor_is_exact(
            a in operand(),
            d in nonzero_denominator(),
        ) {
            prop_assert_eq!(SafeMathRounding::mul_div_floor(a, d, d), Some(a));
            prop_assert_eq!(SafeMathRounding::mul_div_ceil(a, d, d), Some(a));
        }

        #[test]
        fn zero_denominator_is_none(a in any::<i128>(), b in any::<i128>()) {
            prop_assert_eq!(SafeMathRounding::mul_div_floor(a, b, 0), None);
            prop_assert_eq!(SafeMathRounding::mul_div_ceil(a, b, 0), None);
        }

        /// Whole i128 domain: never panics, and returns None whenever the
        /// product overflows.
        #[test]
        fn never_panics_and_overflow_is_none(
            a in any::<i128>(),
            b in any::<i128>(),
            d in any::<i128>(),
        ) {
            let floor = catch_unwind(AssertUnwindSafe(|| SafeMathRounding::mul_div_floor(a, b, d)));
            let ceil = catch_unwind(AssertUnwindSafe(|| SafeMathRounding::mul_div_ceil(a, b, d)));
            prop_assert!(floor.is_ok(), "mul_div_floor({}, {}, {}) panicked", a, b, d);
            prop_assert!(ceil.is_ok(), "mul_div_ceil({}, {}, {}) panicked", a, b, d);
            if a.checked_mul(b).is_none() {
                prop_assert_eq!(floor.unwrap(), None);
                prop_assert_eq!(ceil.unwrap(), None);
            }
        }
    }

    /// The i128::MIN / -1 edge, which uniform sampling is unlikely to hit:
    /// the product fits but the division overflows.
    #[test]
    fn min_over_minus_one_is_none_not_a_panic() {
        assert_eq!(SafeMathRounding::mul_div_floor(i128::MIN, 1, -1), None);
        assert_eq!(SafeMathRounding::mul_div_ceil(i128::MIN, 1, -1), None);
    }
}
