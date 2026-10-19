/// floor(a × b / c) for non-negative operands. `None` on overflow or
/// c == 0.
pub fn mul_div_floor(a: i128, b: i128, c: i128) -> Option<i128> {
    a.checked_mul(b)?.checked_div(c)
}

/// Property-based tests; see README "Property tests".
#[cfg(test)]
mod proptests {
    extern crate std;

    use super::*;
    use proptest::prelude::*;

    const CASES: u32 = 10_000;

    /// Non-negative operands (the documented domain) whose product fits.
    fn operand() -> impl Strategy<Value = i128> {
        0i128..=i64::MAX as i128
    }

    fn divisor() -> impl Strategy<Value = i128> {
        1i128..=i64::MAX as i128
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn is_the_exact_floor(a in operand(), b in operand(), c in divisor()) {
            let q = mul_div_floor(a, b, c).unwrap();
            prop_assert!(q * c <= a * b && a * b < (q + 1) * c);
        }

        #[test]
        fn operands_commute(a in operand(), b in operand(), c in divisor()) {
            prop_assert_eq!(mul_div_floor(a, b, c), mul_div_floor(b, a, c));
        }

        #[test]
        fn is_monotonic_in_each_argument(
            a1 in operand(),
            a2 in operand(),
            b in operand(),
            c1 in divisor(),
            c2 in divisor(),
        ) {
            let (lo, hi) = if a1 <= a2 { (a1, a2) } else { (a2, a1) };
            prop_assert!(mul_div_floor(lo, b, c1).unwrap() <= mul_div_floor(hi, b, c1).unwrap());
            // Non-increasing in the divisor.
            let (lo, hi) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };
            prop_assert!(mul_div_floor(a1, b, lo).unwrap() >= mul_div_floor(a1, b, hi).unwrap());
        }

        /// b <= c means the result never exceeds a (a pro-rata share of a).
        #[test]
        fn a_fraction_of_a_is_bounded_by_a(
            a in operand(),
            (b, c) in divisor().prop_flat_map(|c| (0..=c, Just(c))),
        ) {
            let q = mul_div_floor(a, b, c).unwrap();
            prop_assert!(0 <= q && q <= a);
        }

        /// Whole non-negative domain: the correct value, or `None` exactly
        /// where the product overflows or c == 0, as documented.
        #[test]
        fn fails_exactly_on_overflow_or_zero_divisor(
            a in 0i128..=i128::MAX,
            b in 0i128..=i128::MAX,
            c in 0i128..=i128::MAX,
        ) {
            let expected = a.checked_mul(b).and_then(|p| p.checked_div(c));
            prop_assert_eq!(mul_div_floor(a, b, c), expected);
        }
    }
}
