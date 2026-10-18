//! Outlier filtering and quorum-gated aggregation of feeder reports.
//!
//! Pure functions over a slice of prices: no storage, no `Env`, no
//! allocation, no floats, and no panics on any input. The contract's
//! `get_price` / `aggregate` entrypoints are thin wrappers that collect
//! the fresh reports and call `aggregate` below.
//!
//! # Filter
//!
//! Median-absolute-deviation (MAD) band around the median:
//!
//! 1. `center` = median of the reports (round-down midpoint for even
//!    counts, see `math::median`).
//! 2. `mad` = median of `|report - center|`.
//! 3. A report survives if `|report - center| <= max(k * mad, band)`,
//!    where `band = |center| * min_band_bps / 10_000`.
//!
//! The `band` floor stops the filter from discarding honest reports when
//! most feeders agree exactly (MAD = 0) and one is off by a rounding
//! unit. With fewer than `MIN_FILTER_REPORTS` (3) reports nothing is
//! discarded: with one or two reports there is no majority to say which
//! one is the outlier, so that case is left to the `min_reports` quorum.
//!
//! For `k >= 1` and three or more reports, at least half of the reports
//! (rounded up) are within `mad` of `center`, so a filter can never throw
//! away the honest majority.
//!
//! # Aggregate
//!
//! filter → median of the survivors → quorum: the result is refused with
//! `QuorumNotMet` unless at least `min_reports` reports survive. The
//! price always lies within `[min, max]` of the surviving reports.

use crate::math::{median, median_u128, MAX_FEEDERS};

/// Most reports one aggregation handles — one per authorized feeder.
pub const MAX_REPORTS: usize = MAX_FEEDERS as usize;

/// Below this many reports the filter keeps everything.
pub const MIN_FILTER_REPORTS: usize = 3;

/// Default MAD multiplier: a report more than 3 MADs from the median is
/// an outlier.
pub const DEFAULT_MAD_K: u32 = 3;

/// Default band floor: reports within 1% of the median always survive.
pub const DEFAULT_MIN_BAND_BPS: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterConfig {
    /// MAD multiplier.
    pub k: u32,
    /// Band floor in basis points of `|center|`.
    pub min_band_bps: u32,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            k: DEFAULT_MAD_K,
            min_band_bps: DEFAULT_MIN_BAND_BPS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterError {
    NoReports,
    /// More than `MAX_REPORTS` reports — more than there can be feeders.
    TooManyReports,
}

/// The reports that survived the filter, sorted ascending, plus the
/// statistics the decision was made on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filtered {
    kept: [i128; MAX_REPORTS],
    len: usize,
    /// Median of all the input reports.
    pub center: i128,
    /// Median absolute deviation from `center`.
    pub mad: u128,
    /// Largest deviation from `center` that survived (`u128::MAX` when
    /// there were too few reports to filter).
    pub threshold: u128,
}

impl Filtered {
    pub fn kept(&self) -> &[i128] {
        &self.kept[..self.len]
    }
}

/// Drops outliers from `reports` (see the module docs).
pub fn filter_outliers(reports: &[i128], cfg: FilterConfig) -> Result<Filtered, FilterError> {
    let n = reports.len();
    if n == 0 {
        return Err(FilterError::NoReports);
    }
    if n > MAX_REPORTS {
        return Err(FilterError::TooManyReports);
    }

    let mut sorted = [0i128; MAX_REPORTS];
    sorted[..n].copy_from_slice(reports);
    let center = median(&mut sorted[..n]).ok_or(FilterError::NoReports)?;

    let mut deviations = [0u128; MAX_REPORTS];
    for (d, r) in deviations.iter_mut().zip(sorted[..n].iter()) {
        *d = r.abs_diff(center);
    }
    let mad = median_u128(&mut deviations[..n]).ok_or(FilterError::NoReports)?;

    let threshold = if n < MIN_FILTER_REPORTS {
        u128::MAX
    } else {
        let spread = mad.saturating_mul(cfg.k as u128);
        let band = center
            .unsigned_abs()
            .saturating_mul(cfg.min_band_bps as u128)
            / 10_000;
        spread.max(band)
    };

    let mut kept = [0i128; MAX_REPORTS];
    let mut len = 0;
    for r in sorted[..n].iter() {
        if r.abs_diff(center) <= threshold {
            kept[len] = *r;
            len += 1;
        }
    }
    Ok(Filtered {
        kept,
        len,
        center,
        mad,
        threshold,
    })
}

/// A successful aggregation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aggregate {
    /// Median of the surviving reports.
    pub price: i128,
    /// Reports that survived the filter (>= `min_reports`).
    pub used: u32,
    /// Reports the filter discarded as outliers.
    pub discarded: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggregateError {
    NoReports,
    TooManyReports,
    /// Fewer than `min_reports` reports survived the filter.
    QuorumNotMet {
        survivors: u32,
    },
}

/// filter → median → quorum. `min_reports` of 0 is treated as 1.
pub fn aggregate(
    reports: &[i128],
    min_reports: u32,
    cfg: FilterConfig,
) -> Result<Aggregate, AggregateError> {
    let filtered = filter_outliers(reports, cfg).map_err(|e| match e {
        FilterError::NoReports => AggregateError::NoReports,
        FilterError::TooManyReports => AggregateError::TooManyReports,
    })?;
    let kept = filtered.kept();
    // Both counts are <= MAX_REPORTS, so these casts can't truncate.
    let used = kept.len() as u32;
    let discarded = (reports.len() - kept.len()) as u32;
    if used < min_reports.max(1) {
        return Err(AggregateError::QuorumNotMet { survivors: used });
    }
    let mut buffer = [0i128; MAX_REPORTS];
    buffer[..kept.len()].copy_from_slice(kept);
    let price = median(&mut buffer[..kept.len()]).ok_or(AggregateError::NoReports)?;
    Ok(Aggregate {
        price,
        used,
        discarded,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    fn agg(reports: &[i128], min_reports: u32) -> Result<Aggregate, AggregateError> {
        aggregate(reports, min_reports, FilterConfig::default())
    }

    #[test]
    fn empty_and_oversized_inputs_are_typed_errors() {
        assert_eq!(agg(&[], 1), Err(AggregateError::NoReports));
        assert_eq!(
            agg(&[1; MAX_REPORTS + 1], 1),
            Err(AggregateError::TooManyReports)
        );
    }

    #[test]
    fn one_fresh_report_with_min_reports_one_aggregates() {
        assert_eq!(
            agg(&[1_200_000], 1),
            Ok(Aggregate {
                price: 1_200_000,
                used: 1,
                discarded: 0
            })
        );
    }

    #[test]
    fn single_feeder_dominance_fails_closed() {
        // Every other feeder went stale or was removed: one report left
        // must not be enough once the quorum asks for more.
        assert_eq!(
            agg(&[1_200_000], 3),
            Err(AggregateError::QuorumNotMet { survivors: 1 })
        );
    }

    #[test]
    fn two_reports_are_never_filtered() {
        let r = agg(&[1_000_000, 9_000_000], 2).unwrap();
        assert_eq!(r.used, 2);
        assert_eq!(r.price, 5_000_000);
    }

    #[test]
    fn a_wild_report_is_dropped_before_the_median() {
        // Three honest feeders near 1.00 and one reporting 50x.
        let r = agg(&[10_000_000, 10_010_000, 9_990_000, 500_000_000], 1).unwrap();
        assert_eq!(r.discarded, 1);
        assert_eq!(r.used, 3);
        assert_eq!(r.price, 10_000_000);
    }

    #[test]
    fn identical_reports_all_survive() {
        let r = agg(&[7_000_000; 5], 5).unwrap();
        assert_eq!(
            r,
            Aggregate {
                price: 7_000_000,
                used: 5,
                discarded: 0
            }
        );
    }

    #[test]
    fn band_keeps_small_disagreements_when_mad_is_zero() {
        // MAD is 0 (three identical reports); the 1% band still keeps a
        // fourth report a few units off.
        let r = agg(&[10_000_000, 10_000_000, 10_000_000, 10_000_050], 4).unwrap();
        assert_eq!(r.used, 4);
    }

    #[test]
    fn filtering_down_to_exactly_min_reports_succeeds() {
        let reports = [10_000_000, 10_010_000, 9_990_000, 500_000_000];
        assert_eq!(agg(&reports, 3).unwrap().used, 3);
    }

    #[test]
    fn filtering_down_to_min_reports_minus_one_fails() {
        let reports = [10_000_000, 10_010_000, 9_990_000, 500_000_000];
        assert_eq!(
            agg(&reports, 4),
            Err(AggregateError::QuorumNotMet { survivors: 3 })
        );
    }

    #[test]
    fn table_feeder_counts_1_2_3_5() {
        // (reports, expected price, expected survivors)
        let cases: [(&[i128], i128, u32); 6] = [
            (&[1_000_000], 1_000_000, 1),
            (&[1_000_000, 1_000_003], 1_000_001, 2),
            (&[1_000_000, 1_002_000, 1_001_000], 1_001_000, 3),
            (&[1_000_000, 1_001_000, 9_000_000], 1_000_500, 2),
            (
                &[1_000_000, 1_004_000, 1_002_000, 998_000, 1_001_000],
                1_001_000,
                5,
            ),
            (
                &[1_000_000, 1_004_000, 1_002_000, 998_000, 10],
                1_001_000,
                4,
            ),
        ];
        for (reports, price, used) in cases {
            let r = agg(reports, 1).unwrap();
            assert_eq!((r.price, r.used), (price, used), "{:?}", reports);
        }
    }

    #[test]
    fn extreme_inputs_do_not_panic() {
        let _ = agg(&[i128::MIN, i128::MAX, 0], 1);
        let _ = agg(&[i128::MAX; MAX_REPORTS], 1);
        let _ = aggregate(
            &[i128::MIN, 1, i128::MAX],
            u32::MAX,
            FilterConfig {
                k: u32::MAX,
                min_band_bps: u32::MAX,
            },
        );
    }
}

/// Property-based tests; see README "Property tests".
#[cfg(test)]
mod proptests {
    extern crate std;

    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::vec::Vec;

    const CASES: u32 = 10_000;

    /// Prices from 1e3 to 1e15.
    fn price() -> impl Strategy<Value = i128> {
        1_000i128..=1_000_000_000_000_000
    }

    fn reports() -> impl Strategy<Value = Vec<i128>> {
        vec(price(), 1..=MAX_REPORTS)
    }

    fn k() -> impl Strategy<Value = u32> {
        1u32..=10
    }

    fn cfg(k: u32) -> FilterConfig {
        FilterConfig {
            k,
            min_band_bps: DEFAULT_MIN_BAND_BPS,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        /// The issue's core property: the aggregate always lies within
        /// [min, max] of the surviving reports.
        #[test]
        fn aggregate_is_within_the_surviving_range(values in reports(), k in k()) {
            let filtered = filter_outliers(&values, cfg(k)).unwrap();
            let kept = filtered.kept();
            let a = aggregate(&values, 1, cfg(k)).unwrap();
            let min = *kept.iter().min().unwrap();
            let max = *kept.iter().max().unwrap();
            prop_assert!(min <= a.price && a.price <= max, "{} outside [{}, {}]", a.price, min, max);
            prop_assert_eq!(a.used as usize, kept.len());
            prop_assert_eq!((a.used + a.discarded) as usize, values.len());
        }

        #[test]
        fn survivors_are_a_subset_of_the_input(values in reports(), k in k()) {
            let filtered = filter_outliers(&values, cfg(k)).unwrap();
            for v in filtered.kept() {
                prop_assert!(values.contains(v));
            }
        }

        #[test]
        fn is_permutation_invariant(
            (values, shuffled) in reports()
                .prop_flat_map(|v| (Just(v.clone()), Just(v).prop_shuffle())),
            k in k(),
        ) {
            prop_assert_eq!(aggregate(&values, 1, cfg(k)), aggregate(&shuffled, 1, cfg(k)));
        }

        #[test]
        fn of_a_constant_input_is_that_value(p in price(), n in 1..=MAX_REPORTS, k in k()) {
            prop_assert_eq!(
                aggregate(&std::vec![p; n], n as u32, cfg(k)),
                Ok(Aggregate { price: p, used: n as u32, discarded: 0 })
            );
        }

        /// For k >= 1 at least half the reports (rounded up) survive.
        #[test]
        fn keeps_at_least_half(values in reports(), k in k()) {
            let a = aggregate(&values, 1, cfg(k)).unwrap();
            prop_assert!(2 * a.used as usize >= values.len());
        }

        /// A looser threshold never drops more reports.
        #[test]
        fn is_monotonic_in_k(values in reports(), k1 in k(), k2 in k()) {
            let (lo, hi) = if k1 <= k2 { (k1, k2) } else { (k2, k1) };
            let strict = aggregate(&values, 1, cfg(lo)).unwrap().used;
            let loose = aggregate(&values, 1, cfg(hi)).unwrap().used;
            prop_assert!(strict <= loose);
        }

        /// Without the band floor (which scales with the price level),
        /// shifting every report by c shifts the price by exactly c and
        /// keeps the same number of reports.
        #[test]
        fn is_translation_equivariant_without_band(
            values in reports(),
            c in 0i128..=1_000_000_000_000_000,
            k in k(),
        ) {
            let no_band = FilterConfig { k, min_band_bps: 0 };
            let shifted: Vec<i128> = values.iter().map(|v| v + c).collect();
            let a0 = aggregate(&values, 1, no_band).unwrap();
            let a1 = aggregate(&shifted, 1, no_band).unwrap();
            prop_assert_eq!(a1.price, a0.price + c);
            prop_assert_eq!(a1.used, a0.used);
        }

        /// Quorum is exactly "survivors >= min_reports".
        #[test]
        fn quorum_boundary_is_exact(values in reports(), k in k(), min in 1u32..=20) {
            let survivors = filter_outliers(&values, cfg(k)).unwrap().kept().len() as u32;
            let result = aggregate(&values, min, cfg(k));
            if survivors >= min {
                prop_assert!(result.is_ok());
            } else {
                prop_assert_eq!(result, Err(AggregateError::QuorumNotMet { survivors }));
            }
        }

        /// Whole i128 domain and any config: never panics.
        #[test]
        fn never_panics(
            values in vec(any::<i128>(), 0..=MAX_REPORTS + 2),
            k in any::<u32>(),
            band in any::<u32>(),
            min in any::<u32>(),
        ) {
            let config = FilterConfig { k, min_band_bps: band };
            let result = catch_unwind(AssertUnwindSafe(|| aggregate(&values, min, config)));
            prop_assert!(result.is_ok(), "panicked on {:?}", values);
        }
    }
}
