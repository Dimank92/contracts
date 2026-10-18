//! Integer-only order statistics for price aggregation. No floats, no
//! allocation, no panics on any input.

/// Caps how many feeders can be authorized at once, so aggregation (a
/// simple O(n^2) sort over collected reports) stays bounded regardless of
/// how many feeders the admin adds over the contract's lifetime.
pub const MAX_FEEDERS: u32 = 16;

/// Sorts `values` ascending in place. Insertion sort: fine for
/// MAX_FEEDERS-sized input, no allocation needed.
pub fn sort(values: &mut [i128]) {
    for i in 1..values.len() {
        let mut j = i;
        while j > 0 && values[j - 1] > values[j] {
            values.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// `floor((lo + hi) / 2)` for `lo <= hi`, without the overflow
/// `(lo + hi) / 2` hits near the ends of the `i128` range: the gap
/// `hi - lo` always fits in a `u128`, and half of it fits back in an
/// `i128`, so `lo + gap / 2` can't overflow and never exceeds `hi`.
pub fn midpoint_floor(lo: i128, hi: i128) -> i128 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let half_gap = (hi.abs_diff(lo) / 2) as i128;
    lo + half_gap
}

/// Median of `values`, sorting them in place. `None` for an empty slice.
///
/// Rounding: for an even count the median is the midpoint of the two
/// middle values, rounded **down** (toward negative infinity) when that
/// midpoint isn't a whole number — e.g. `[10, 21]` gives `15`. For the
/// positive prices this contract stores that is the same as truncation,
/// and it is the same direction regardless of input order or sign.
pub fn median(values: &mut [i128]) -> Option<i128> {
    let n = values.len();
    if n == 0 {
        return None;
    }
    sort(values);
    if n % 2 == 1 {
        Some(values[n / 2])
    } else {
        Some(midpoint_floor(values[n / 2 - 1], values[n / 2]))
    }
}

/// Median of a sorted-in-place slice of unsigned deviations, with the
/// same round-down rule as `median`. `None` for an empty slice.
pub fn median_u128(values: &mut [u128]) -> Option<u128> {
    let n = values.len();
    if n == 0 {
        return None;
    }
    for i in 1..n {
        let mut j = i;
        while j > 0 && values[j - 1] > values[j] {
            values.swap(j - 1, j);
            j -= 1;
        }
    }
    if n % 2 == 1 {
        Some(values[n / 2])
    } else {
        let (lo, hi) = (values[n / 2 - 1], values[n / 2]);
        Some(lo + (hi - lo) / 2)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn med(values: &[i128]) -> Option<i128> {
        let mut buffer = [0i128; MAX_FEEDERS as usize];
        buffer[..values.len()].copy_from_slice(values);
        median(&mut buffer[..values.len()])
    }

    #[test]
    fn empty_input_has_no_median() {
        assert_eq!(med(&[]), None);
    }

    #[test]
    fn single_value_is_its_own_median() {
        assert_eq!(med(&[42]), Some(42));
    }

    #[test]
    fn odd_count_takes_the_middle_after_sorting() {
        assert_eq!(med(&[5, 1, 3]), Some(3));
    }

    #[test]
    fn even_count_averages_the_two_middle_values() {
        assert_eq!(med(&[10, 20, 30, 40]), Some(25));
    }

    #[test]
    fn even_count_rounds_down_consistently() {
        assert_eq!(med(&[10, 21]), Some(15));
        assert_eq!(med(&[21, 10]), Some(15));
        // Toward negative infinity, not toward zero.
        assert_eq!(med(&[-21, -10]), Some(-16));
    }

    #[test]
    fn ties_at_the_median_are_handled() {
        assert_eq!(med(&[7, 7, 1, 9]), Some(7));
        assert_eq!(med(&[5, 5, 5, 5, 6]), Some(5));
    }

    #[test]
    fn already_sorted_input_is_unaffected() {
        assert_eq!(med(&[1, 2, 3, 4, 5]), Some(3));
    }

    #[test]
    fn a_single_outlier_does_not_move_the_median() {
        // Median resists outliers the way a mean wouldn't — a stale or
        // manipulated report of 1_000_000 shouldn't move this far from 20.
        assert_eq!(med(&[18, 19, 20, 21, 1_000_000]), Some(20));
    }

    #[test]
    fn every_feeder_agreeing_is_unaffected_by_sorting() {
        assert_eq!(med(&[500_000; MAX_FEEDERS as usize]), Some(500_000));
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
        // Sorted 1..=16, even count -> floor of the mean of (8, 9).
        assert_eq!(med(&values), Some(8));
    }

    #[test]
    fn extreme_values_do_not_overflow() {
        assert_eq!(med(&[i128::MAX, i128::MAX]), Some(i128::MAX));
        assert_eq!(med(&[i128::MIN, i128::MIN]), Some(i128::MIN));
        assert_eq!(med(&[i128::MIN, i128::MAX]), Some(-1));
    }

    #[test]
    fn table_feeder_counts_1_2_3_5() {
        let cases: [(&[i128], i128); 4] = [
            (&[1_000], 1_000),
            (&[1_000, 1_003], 1_001),
            (&[1_003, 1_000, 1_002], 1_002),
            (&[1_005, 990, 1_000, 1_010, 995], 1_000),
        ];
        for (values, expected) in cases {
            assert_eq!(med(values), Some(expected), "{:?}", values);
        }
    }

    #[test]
    fn median_u128_matches_the_signed_rounding_rule() {
        assert_eq!(median_u128(&mut []), None);
        assert_eq!(median_u128(&mut [3, 1, 2]), Some(2));
        assert_eq!(median_u128(&mut [10, 21]), Some(15));
        assert_eq!(median_u128(&mut [u128::MAX, u128::MAX]), Some(u128::MAX));
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
        let mut copy = values.to_vec();
        median(&mut copy).unwrap()
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

        /// Shifting every report by c shifts the median by exactly c: the
        /// round-down midpoint depends only on the gap between the two
        /// middle values, which a shift leaves unchanged.
        #[test]
        fn is_translation_equivariant(
            values in reports(),
            c in -1_000_000_000_000_000i128..=1_000_000_000_000_000,
        ) {
            let shifted: Vec<i128> = values.iter().map(|v| v + c).collect();
            prop_assert_eq!(median_of(&shifted), median_of(&values) + c);
        }

        /// A single report, however extreme, can't drag the median outside
        /// the range of the other reports (for 3+ reports).
        #[test]
        fn resists_a_single_outlier(
            values in vec(price(), 3..=CAP),
            outlier in any::<i128>(),
        ) {
            let honest = &values[1..];
            let mut with_outlier = values.clone();
            with_outlier[0] = outlier;
            let m = median_of(&with_outlier);
            let min = *honest.iter().min().unwrap();
            let max = *honest.iter().max().unwrap();
            prop_assert!(min <= m && m <= max);
        }

        /// Whole i128 domain: never panics, and always returns exactly the
        /// floor of the middle value(s) — computed here in wider
        /// arithmetic via two halves, so overflow can't hide a mismatch.
        #[test]
        fn is_exact_on_the_whole_domain(values in vec(any::<i128>(), 1..=CAP)) {
            let mut sorted = values.clone();
            sorted.sort();
            let n = sorted.len();
            let expected = if n % 2 == 1 {
                sorted[n / 2]
            } else {
                let (a, b) = (sorted[n / 2 - 1], sorted[n / 2]);
                // floor((a + b) / 2) == a.div_euclid(2) + b.div_euclid(2)
                //   + (a.rem_euclid(2) + b.rem_euclid(2)) / 2
                a.div_euclid(2) + b.div_euclid(2) + (a.rem_euclid(2) + b.rem_euclid(2)) / 2
            };
            prop_assert_eq!(median_of(&values), expected);
        }
    }
}
