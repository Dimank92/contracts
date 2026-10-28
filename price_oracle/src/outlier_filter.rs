#[cfg(test)]
use soroban_sdk::Env;
use soroban_sdk::Vec;

use crate::math::{abs_diff, sort, MAX_FEEDERS};

pub struct PriceDispersionResult {
    pub filtered_median: i128,
    pub mad: i128,
    pub dispersion_bps: u32,
    pub valid_reports_count: u32,
}

pub struct OutlierFilter;

impl OutlierFilter {
    /// Filters out price reports exceeding `k * MAD` (Median Absolute
    /// Deviation) from the median, mitigating flash crashes and collusive
    /// feeder outliers, and returns the mean of the kept reports.
    ///
    /// "Median" here is the upper median (`sorted[n / 2]`) for both the
    /// price and the deviations, so at least `n / 2 + 1` reports are
    /// always within `MAD` and kept for any `k >= 1`. `None` for an empty
    /// set, more than `MAX_FEEDERS` reports, or arithmetic that would
    /// overflow — never a panic.
    pub fn compute_robust_price(reports: Vec<i128>, k: u32) -> Option<PriceDispersionResult> {
        let n = reports.len() as usize;
        if n == 0 || n > MAX_FEEDERS as usize {
            return None;
        }

        // 1. Sort to determine the preliminary median.
        let mut buffer = [0i128; MAX_FEEDERS as usize];
        for (slot, value) in buffer.iter_mut().zip(reports.iter()) {
            *slot = value;
        }
        let sorted = &mut buffer[..n];
        sort(sorted);
        let median = sorted[n / 2];

        // 2. Median of the absolute deviations from it.
        let mut devs = [0i128; MAX_FEEDERS as usize];
        for (dev, value) in devs.iter_mut().zip(sorted.iter()) {
            *dev = abs_diff(*value, median).ok()?;
        }
        let devs = &mut devs[..n];
        sort(devs);
        let mad = devs[n / 2];

        // 3. Drop reports > k * MAD.
        let threshold = mad.checked_mul(k as i128)?;
        let mut valid_sum = 0i128;
        let mut valid_count = 0u32;
        for value in sorted.iter() {
            if abs_diff(*value, median).ok()? <= threshold {
                valid_sum = valid_sum.checked_add(*value)?;
                valid_count += 1;
            }
        }

        if valid_count == 0 {
            return None;
        }

        let filtered_median = valid_sum / valid_count as i128;
        // Saturates rather than overflowing or truncating: a dispersion
        // past u32::MAX bps is "enormous" either way.
        let dispersion_bps = if filtered_median > 0 {
            mad.checked_mul(10_000)
                .and_then(|scaled| u32::try_from(scaled / filtered_median).ok())
                .unwrap_or(u32::MAX)
        } else {
            0
        };

        Some(PriceDispersionResult {
            filtered_median,
            mad,
            dispersion_bps,
            valid_reports_count: valid_count,
        })
    }
}

/// Property-based tests for MAD aggregation; see README "Property tests".
#[cfg(test)]
mod proptests {
    extern crate std;

    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    // Each case builds a fresh soroban Env, so this is the slowest suite;
    // it still stays well inside the 60s-per-crate CI budget.
    const CASES: u32 = 10_000;
    const MAX_REPORTS: usize = 16;

    /// Prices from 1e3 to 1e15.
    fn price() -> impl Strategy<Value = i128> {
        1_000i128..=1_000_000_000_000_000
    }

    fn reports() -> impl Strategy<Value = std::vec::Vec<i128>> {
        vec(price(), 1..=MAX_REPORTS)
    }

    fn k() -> impl Strategy<Value = u32> {
        1u32..=10
    }

    /// (filtered_median, mad, dispersion_bps, valid_reports_count)
    fn robust(values: &[i128], k: u32) -> Option<(i128, i128, u32, u32)> {
        let env = Env::default();
        OutlierFilter::compute_robust_price(Vec::from_slice(&env, values), k).map(|r| {
            (
                r.filtered_median,
                r.mad,
                r.dispersion_bps,
                r.valid_reports_count,
            )
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn empty_input_has_no_price(k in any::<u32>()) {
            prop_assert_eq!(robust(&[], k), None);
        }

        #[test]
        fn filtered_price_is_bounded_by_min_and_max(values in reports(), k in k()) {
            let (price, mad, _, count) = robust(&values, k).unwrap();
            let min = *values.iter().min().unwrap();
            let max = *values.iter().max().unwrap();
            prop_assert!(min <= price && price <= max, "{} outside [{}, {}]", price, min, max);
            prop_assert!(mad >= 0);
            prop_assert!(count >= 1 && count as usize <= values.len());
        }

        #[test]
        fn is_permutation_invariant(
            (values, shuffled) in reports()
                .prop_flat_map(|v| (Just(v.clone()), Just(v).prop_shuffle())),
            k in k(),
        ) {
            prop_assert_eq!(robust(&values, k), robust(&shuffled, k));
        }

        #[test]
        fn of_a_constant_input_is_that_value(p in price(), n in 1..=MAX_REPORTS, k in k()) {
            prop_assert_eq!(robust(&std::vec![p; n], k), Some((p, 0, 0, n as u32)));
        }

        /// MAD is the median absolute deviation, so for k >= 1 more than
        /// half the reports are always within the threshold and kept.
        #[test]
        fn keeps_a_majority_of_reports(values in reports(), k in k()) {
            let (_, _, _, count) = robust(&values, k).unwrap();
            prop_assert!(count as usize > values.len() / 2);
        }

        /// A looser threshold never drops more reports.
        #[test]
        fn is_monotonic_in_k(values in reports(), k1 in k(), k2 in k()) {
            let (lo, hi) = if k1 <= k2 { (k1, k2) } else { (k2, k1) };
            let (_, _, _, strict) = robust(&values, lo).unwrap();
            let (_, _, _, loose) = robust(&values, hi).unwrap();
            prop_assert!(strict <= loose);
        }

        /// Shifting every report by c shifts the price by exactly c and
        /// leaves MAD and the kept set unchanged.
        #[test]
        fn is_translation_equivariant(
            values in reports(),
            c in 0i128..=1_000_000_000_000_000,
            k in k(),
        ) {
            let shifted: std::vec::Vec<i128> = values.iter().map(|v| v + c).collect();
            let (p0, mad0, _, n0) = robust(&values, k).unwrap();
            let (p1, mad1, _, n1) = robust(&shifted, k).unwrap();
            prop_assert_eq!(p1, p0 + c);
            prop_assert_eq!(mad1, mad0);
            prop_assert_eq!(n1, n0);
        }

        /// Whole i128 domain: returns Some or None, never panics.
        #[test]
        fn never_panics(values in vec(any::<i128>(), 0..=MAX_REPORTS), k in any::<u32>()) {
            let result = catch_unwind(AssertUnwindSafe(|| robust(&values, k)));
            prop_assert!(result.is_ok(), "panicked on {:?} k={}", values, k);
        }
    }
}
