//! Fixed-point conventions (enforced by `cargo xtask check-determinism`, which
//! rejects f32/f64, float literals, and std time/env/HashMap in non-test
//! contract code): prices are `i128` at PRICE_PRECISION (7 decimals);
//! division truncates toward zero unless a function documents otherwise; all
//! overflow is checked (`checked_*`, or `overflow-checks = true` in release
//! so it panics rather than wraps).

/// Caps how many feeders can be authorized at once, so aggregation (a
/// simple O(n^2) sort over collected reports) stays bounded regardless of
/// how many feeders the admin adds over the contract's lifetime.
pub const MAX_FEEDERS: u32 = 16;

/// Median of a small, fixed-size buffer of fresh feeder prices. `count` is
/// how many of the leading entries in `values` are actually populated —
/// the buffer itself is always MAX_FEEDERS-sized regardless of how many
/// feeders are currently fresh, since it's built on the stack rather than
/// a heap-allocated Vec. Panics on `count == 0`; callers must check for
/// "no fresh reports" before calling this.
pub fn median(values: &mut [i128; MAX_FEEDERS as usize], count: usize) -> i128 {
    assert!(count > 0);
    // Insertion sort: fine for MAX_FEEDERS-sized input, no allocation needed.
    for i in 1..count {
        let mut j = i;
        while j > 0 && values[j - 1] > values[j] {
            values.swap(j - 1, j);
            j -= 1;
        }
    }
    if count % 2 == 1 {
        values[count / 2]
    } else {
        values[count / 2 - 1]
            .checked_add(values[count / 2])
            .unwrap()
            / 2
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn buf(values: &[i128]) -> ([i128; MAX_FEEDERS as usize], usize) {
        let mut buffer = [0i128; MAX_FEEDERS as usize];
        for (i, v) in values.iter().enumerate() {
            buffer[i] = *v;
        }
        (buffer, values.len())
    }

    #[test]
    fn single_value_is_its_own_median() {
        let (mut b, n) = buf(&[42]);
        assert_eq!(median(&mut b, n), 42);
    }

    #[test]
    fn odd_count_takes_the_middle_after_sorting() {
        let (mut b, n) = buf(&[5, 1, 3]);
        assert_eq!(median(&mut b, n), 3);
    }

    #[test]
    fn even_count_averages_the_two_middle_values() {
        let (mut b, n) = buf(&[10, 20, 30, 40]);
        assert_eq!(median(&mut b, n), 25);
    }

    #[test]
    fn already_sorted_input_is_unaffected() {
        let (mut b, n) = buf(&[1, 2, 3, 4, 5]);
        assert_eq!(median(&mut b, n), 3);
    }

    #[test]
    fn a_single_outlier_does_not_move_the_median() {
        // Median resists outliers the way a mean wouldn't — a stale or
        // manipulated report of 1_000_000 shouldn't move this far from 20.
        let (mut b, n) = buf(&[18, 19, 20, 21, 1_000_000]);
        assert_eq!(median(&mut b, n), 20);
    }

    #[test]
    fn every_feeder_agreeing_is_unaffected_by_sorting() {
        let (mut b, n) = buf(&[500_000; MAX_FEEDERS as usize]);
        assert_eq!(median(&mut b, n), 500_000);
    }

    #[test]
    fn a_full_buffer_of_max_feeders_is_handled() {
        // MAX_FEEDERS (16, even) descending, to exercise both the sort and
        // the even-count averaging branch at the actual capacity this
        // function is sized for.
        let mut values = [0i128; MAX_FEEDERS as usize];
        for (i, v) in values.iter_mut().enumerate() {
            *v = MAX_FEEDERS as i128 - i as i128;
        }
        let (mut b, n) = buf(&values);
        // Sorted 1..=16, even count -> average of the two middle (8, 9).
        assert_eq!(median(&mut b, n), 8);
    }
}

/// Property-based tests; see README "Property tests". Shrunk failures are
/// persisted under `proptest-regressions/` and must be committed.
#[cfg(test)]
mod proptests {
    extern crate std;

    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::vec::Vec;

    const CASES: u32 = 10_000;
    const CAP: usize = MAX_FEEDERS as usize;

    /// Prices from 1e3 to 1e15.
    fn price() -> impl Strategy<Value = i128> {
        1_000i128..=1_000_000_000_000_000
    }

    /// 1..=MAX_FEEDERS fresh reports.
    fn reports() -> impl Strategy<Value = Vec<i128>> {
        vec(price(), 1..=CAP)
    }

    fn median_of(values: &[i128]) -> i128 {
        let mut buffer = [0i128; CAP];
        buffer[..values.len()].copy_from_slice(values);
        median(&mut buffer, values.len())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn is_bounded_by_min_and_max(values in reports()) {
            let m = median_of(&values);
            let min = *values.iter().min().unwrap();
            let max = *values.iter().max().unwrap();
            prop_assert!(min <= m && m <= max, "{} outside [{}, {}]", m, min, max);
        }

        #[test]
        fn is_permutation_invariant(
            (values, shuffled) in reports()
                .prop_flat_map(|v| (Just(v.clone()), Just(v).prop_shuffle())),
        ) {
            prop_assert_eq!(median_of(&values), median_of(&shuffled));
        }

        #[test]
        fn of_a_constant_input_is_that_value(p in price(), n in 1..=CAP) {
            prop_assert_eq!(median_of(&std::vec![p; n]), p);
        }

        /// At least half the reports are <= the median and at least half
        /// are >= it — the defining property of a median.
        #[test]
        fn splits_the_reports_in_half(values in reports()) {
            let m = median_of(&values);
            let n = values.len();
            let below = values.iter().filter(|v| **v <= m).count();
            let above = values.iter().filter(|v| **v >= m).count();
            prop_assert!(2 * below >= n && 2 * above >= n);
        }

        /// Raising one report never lowers the median.
        #[test]
        fn is_monotonic_in_each_report(
            (values, i) in reports()
                .prop_flat_map(|v| { let n = v.len(); (Just(v), 0..n) }),
            bump in 0i128..=1_000_000_000_000_000,
        ) {
            let mut raised = values.clone();
            raised[i] += bump;
            prop_assert!(median_of(&values) <= median_of(&raised));
        }

        /// Shifting every report by c shifts the median by exactly c: for
        /// positive values the even-count average truncates the same way
        /// before and after, since the sum moves by 2c.
        #[test]
        fn is_translation_equivariant(
            values in reports(),
            c in 0i128..=1_000_000_000_000_000,
        ) {
            let shifted: Vec<i128> = values.iter().map(|v| v + c).collect();
            prop_assert_eq!(median_of(&shifted), median_of(&values) + c);
        }

        /// A single report, however extreme, can't drag the median outside
        /// the range of the other reports (for 3+ reports).
        #[test]
        fn resists_a_single_outlier(
            values in vec(price(), 3..=CAP),
            outlier in any::<i64>().prop_map(i128::from),
        ) {
            let honest = &values[1..];
            let mut with_outlier = values.clone();
            with_outlier[0] = outlier;
            let m = median_of(&with_outlier);
            let min = *honest.iter().min().unwrap();
            let max = *honest.iter().max().unwrap();
            prop_assert!(min <= m && m <= max);
        }

        /// Whole i128 domain: either the correct value or a failure exactly
        /// where averaging the two middle values overflows. Once the
        /// typed-error work lands this becomes Ok/Err with no panic.
        #[test]
        fn overflow_is_never_silent(values in vec(any::<i128>(), 1..=CAP)) {
            let mut sorted = values.clone();
            sorted.sort();
            let n = sorted.len();
            let expected = if n % 2 == 1 {
                Some(sorted[n / 2])
            } else {
                sorted[n / 2 - 1].checked_add(sorted[n / 2]).map(|s| s / 2)
            };
            let actual = catch_unwind(AssertUnwindSafe(|| median_of(&values))).ok();
            prop_assert_eq!(actual, expected);
        }
    }
}
