//! Fixed-point price representation and the numeric primitives the
//! aggregation paths (`oracle_adapter`, `outlier_filter`) build on.
//!
//! Pure: no storage, no `Env`. Every operation is checked and returns a
//! typed [`MathError`] instead of panicking, whatever the input.
//!
//! # Price representation
//!
//! A price is an `i128` fixed-point number with [`PRICE_DECIMALS`] (7)
//! implied decimals, so `1.0 == PRICE_SCALE == 10_000_000` — the same
//! `PRICE_PRECISION` options_market settles in, so an aggregated price is
//! handed to it unconverted. Reports that arrive at another precision are
//! normalized with [`rescale`] first. Each symbol additionally declares
//! how many of those 7 decimals are meaningful (see
//! [`check_precision`]).
//!
//! # Rounding
//!
//! Every operation that can lose precision states its rounding direction:
//!
//! | Operation          | Rounding                                          |
//! |--------------------|---------------------------------------------------|
//! | [`median`]         | even count: floor of the two middle values' mean  |
//! | [`rescale`]        | caller picks [`Rounding::Floor`] / [`Rounding::Ceil`] |
//! | [`deviation_bps`]  | ceil (a borderline report counts as deviating)    |
//! | [`abs_diff`]       | exact                                             |

/// Caps how many feeders can be authorized at once, so aggregation (an
/// O(n^2) insertion sort over collected reports) stays bounded regardless
/// of how many feeders the admin adds over the contract's lifetime.
pub const MAX_FEEDERS: u32 = 16;

/// Implied decimals of every stored and aggregated price.
pub const PRICE_DECIMALS: u32 = 7;

/// `10^PRICE_DECIMALS`: the fixed-point representation of 1.0.
pub const PRICE_SCALE: i128 = 10_000_000;

/// Largest precision [`rescale`] accepts on either side. `10^38` is the
/// last power of ten that fits an `i128`, but nothing real quotes past 18.
pub const MAX_DECIMALS: u32 = 18;

/// Largest price the oracle will accept: one trillion whole units. Far
/// above any real quote for the listed assets, and low enough that
/// options_market's `price * contracts` products can't overflow.
pub const MAX_PRICE: i128 = 1_000_000_000_000 * PRICE_SCALE;

/// Basis points in 100%.
pub const BPS_DENOMINATOR: u32 = 10_000;

/// Why a primitive could not produce a value.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MathError {
    /// An aggregate over zero values was requested.
    EmptySet,
    /// The exact result does not fit the return type.
    Overflow,
    /// A precision above [`MAX_DECIMALS`] (or above [`PRICE_DECIMALS`]
    /// where a symbol precision is expected).
    InvalidDecimals,
    /// A ratio against a non-positive reference.
    InvalidReference,
}

/// Rounding direction for operations that can lose precision.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Rounding {
    /// Toward negative infinity.
    Floor,
    /// Toward positive infinity.
    Ceil,
}

/// `10^decimals`, for `decimals <= MAX_DECIMALS`.
pub fn pow10(decimals: u32) -> Result<i128, MathError> {
    if decimals > MAX_DECIMALS {
        return Err(MathError::InvalidDecimals);
    }
    10i128.checked_pow(decimals).ok_or(MathError::Overflow)
}

/// Converts `value` from `from_decimals` implied decimals to
/// `to_decimals`. Scaling up is exact (or `Overflow`); scaling down
/// divides and rounds in the given direction.
pub fn rescale(
    value: i128,
    from_decimals: u32,
    to_decimals: u32,
    rounding: Rounding,
) -> Result<i128, MathError> {
    if from_decimals > MAX_DECIMALS || to_decimals > MAX_DECIMALS {
        return Err(MathError::InvalidDecimals);
    }
    if to_decimals >= from_decimals {
        let factor = pow10(to_decimals - from_decimals)?;
        return value.checked_mul(factor).ok_or(MathError::Overflow);
    }
    let factor = pow10(from_decimals - to_decimals)?;
    // factor > 0, so div_euclid/rem_euclid can't overflow or trap.
    let floor = value.div_euclid(factor);
    match rounding {
        Rounding::Floor => Ok(floor),
        Rounding::Ceil if value.rem_euclid(factor) != 0 => {
            floor.checked_add(1).ok_or(MathError::Overflow)
        }
        Rounding::Ceil => Ok(floor),
    }
}

/// Normalizes a price quoted with `decimals` implied decimals to the
/// oracle's [`PRICE_DECIMALS`], rounding down. Floor rather than nearest
/// so normalization never invents value the source didn't report.
pub fn normalize_price(value: i128, decimals: u32) -> Result<i128, MathError> {
    rescale(value, decimals, PRICE_DECIMALS, Rounding::Floor)
}

/// True if `price` (at [`PRICE_DECIMALS`]) carries no more than
/// `symbol_precision` meaningful decimals, i.e. is a whole multiple of
/// `10^(PRICE_DECIMALS - symbol_precision)`.
pub fn check_precision(price: i128, symbol_precision: u32) -> Result<bool, MathError> {
    if symbol_precision > PRICE_DECIMALS {
        return Err(MathError::InvalidDecimals);
    }
    let step = pow10(PRICE_DECIMALS - symbol_precision)?;
    Ok(price.rem_euclid(step) == 0)
}

/// Floor of `(a + b) / 2`, without the intermediate sum (which can
/// overflow for two large values of the same sign).
pub fn floor_mean(a: i128, b: i128) -> i128 {
    // Each half is at most |i128::MIN / 2|, the remainders sum to 0..=2,
    // so nothing here can overflow.
    a.div_euclid(2) + b.div_euclid(2) + (a.rem_euclid(2) + b.rem_euclid(2)) / 2
}

/// Sorts ascending in place. Insertion sort: no allocation, and every
/// caller's input is at most [`MAX_FEEDERS`] long.
pub fn sort(values: &mut [i128]) {
    for i in 1..values.len() {
        let mut j = i;
        while j > 0 && values[j - 1] > values[j] {
            values.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// Median of `values`, sorting the slice in place (insertion sort: no
/// allocation, and inputs are at most [`MAX_FEEDERS`] long).
///
/// Odd count: the middle value. Even count (the tie-breaking rule): the
/// floor of the mean of the two middle values — for positive prices that
/// is the usual truncating average. Duplicates need no special handling;
/// the result is always within `[min, max]` of the input.
pub fn median(values: &mut [i128]) -> Result<i128, MathError> {
    let n = values.len();
    if n == 0 {
        return Err(MathError::EmptySet);
    }
    sort(values);
    if n % 2 == 1 {
        Ok(values[n / 2])
    } else {
        Ok(floor_mean(values[n / 2 - 1], values[n / 2]))
    }
}

/// `|a - b|`, exact. `Overflow` only when the distance itself exceeds
/// `i128::MAX` (operands of opposite sign near the extremes).
pub fn abs_diff(a: i128, b: i128) -> Result<i128, MathError> {
    i128::try_from(a.abs_diff(b)).map_err(|_| MathError::Overflow)
}

/// How far `value` is from `reference`, in basis points of `reference`,
/// rounded up and saturating at `u32::MAX` (anything that large is
/// "enormous" for every caller). `reference` must be positive.
pub fn deviation_bps(value: i128, reference: i128) -> Result<u32, MathError> {
    if reference <= 0 {
        return Err(MathError::InvalidReference);
    }
    let diff = value.abs_diff(reference);
    let Some(scaled) = diff.checked_mul(BPS_DENOMINATOR as u128) else {
        return Ok(u32::MAX);
    };
    let reference = reference as u128; // positive, checked above
    let bps = scaled.div_ceil(reference);
    Ok(u32::try_from(bps).unwrap_or(u32::MAX))
}

/// True if `value` is within `max_bps` of `reference` (inclusive).
pub fn within_deviation(value: i128, reference: i128, max_bps: u32) -> Result<bool, MathError> {
    Ok(deviation_bps(value, reference)? <= max_bps)
}

#[cfg(test)]
mod test {
    use super::*;

    fn med(values: &[i128]) -> Result<i128, MathError> {
        let mut buffer = [0i128; MAX_FEEDERS as usize];
        buffer[..values.len()].copy_from_slice(values);
        median(&mut buffer[..values.len()])
    }

    #[test]
    fn empty_set_is_a_typed_error() {
        assert_eq!(med(&[]), Err(MathError::EmptySet));
    }

    #[test]
    fn single_value_is_its_own_median() {
        assert_eq!(med(&[42]), Ok(42));
    }

    #[test]
    fn odd_count_takes_the_middle_after_sorting() {
        assert_eq!(med(&[5, 1, 3]), Ok(3));
    }

    #[test]
    fn even_count_averages_the_two_middle_values() {
        assert_eq!(med(&[10, 20, 30, 40]), Ok(25));
    }

    #[test]
    fn even_count_tie_break_floors_the_mean() {
        assert_eq!(med(&[10, 11]), Ok(10));
        assert_eq!(med(&[-11, -10]), Ok(-11));
    }

    #[test]
    fn duplicates_are_counted_individually() {
        assert_eq!(med(&[7, 7, 7, 100]), Ok(7));
        assert_eq!(med(&[1, 7, 7]), Ok(7));
    }

    #[test]
    fn already_sorted_input_is_unaffected() {
        assert_eq!(med(&[1, 2, 3, 4, 5]), Ok(3));
    }

    #[test]
    fn a_single_outlier_does_not_move_the_median() {
        // A stale or manipulated report of 1_000_000 shouldn't move this
        // far from 20, the way it would move a mean.
        assert_eq!(med(&[18, 19, 20, 21, 1_000_000]), Ok(20));
    }

    #[test]
    fn every_feeder_agreeing_is_unaffected_by_sorting() {
        assert_eq!(med(&[500_000; MAX_FEEDERS as usize]), Ok(500_000));
    }

    #[test]
    fn a_full_buffer_of_max_feeders_is_handled() {
        let mut values = [0i128; MAX_FEEDERS as usize];
        for (i, v) in values.iter_mut().enumerate() {
            *v = MAX_FEEDERS as i128 - i as i128;
        }
        // Sorted 1..=16, even count -> floor mean of the middle (8, 9).
        assert_eq!(med(&values), Ok(8));
    }

    #[test]
    fn maximum_value_reports_do_not_overflow() {
        assert_eq!(med(&[i128::MAX, i128::MAX]), Ok(i128::MAX));
        assert_eq!(med(&[i128::MIN, i128::MIN]), Ok(i128::MIN));
        assert_eq!(med(&[i128::MIN, i128::MAX]), Ok(-1));
        assert_eq!(med(&[i128::MAX - 1, i128::MAX]), Ok(i128::MAX - 1));
    }

    #[test]
    fn rescale_up_is_exact() {
        assert_eq!(rescale(12, 1, 7, Rounding::Floor), Ok(12_000_000));
        assert_eq!(rescale(5, 7, 7, Rounding::Ceil), Ok(5));
    }

    #[test]
    fn rescale_down_rounds_as_asked() {
        assert_eq!(
            rescale(1_234_567_891, 9, 7, Rounding::Floor),
            Ok(12_345_678)
        );
        assert_eq!(rescale(1_234_567_891, 9, 7, Rounding::Ceil), Ok(12_345_679));
        assert_eq!(rescale(1_234_567_800, 9, 7, Rounding::Ceil), Ok(12_345_678));
        assert_eq!(rescale(-15, 1, 0, Rounding::Floor), Ok(-2));
        assert_eq!(rescale(-15, 1, 0, Rounding::Ceil), Ok(-1));
    }

    #[test]
    fn rescale_rejects_bad_precision_and_overflow() {
        assert_eq!(
            rescale(1, 19, 7, Rounding::Floor),
            Err(MathError::InvalidDecimals)
        );
        assert_eq!(
            rescale(1, 7, 19, Rounding::Floor),
            Err(MathError::InvalidDecimals)
        );
        assert_eq!(
            rescale(i128::MAX, 0, 18, Rounding::Floor),
            Err(MathError::Overflow)
        );
        assert_eq!(
            rescale(i128::MAX, 18, 0, Rounding::Ceil),
            Ok(i128::MAX / 10i128.pow(18) + 1)
        );
    }

    #[test]
    fn the_same_price_at_different_precisions_normalizes_identically() {
        let at_7 = 1_200_000; // 0.12
        assert_eq!(normalize_price(12, 2), Ok(at_7));
        assert_eq!(normalize_price(120_000_000_000_000_000, 18), Ok(at_7));
        assert_eq!(normalize_price(at_7, 7), Ok(at_7));
    }

    #[test]
    fn precision_check_enforces_the_symbols_declared_decimals() {
        assert_eq!(check_precision(1_230_000, 5), Ok(true));
        assert_eq!(check_precision(1_234_567, 5), Ok(false));
        assert_eq!(check_precision(1_234_567, 7), Ok(true));
        assert_eq!(check_precision(1, 8), Err(MathError::InvalidDecimals));
    }

    #[test]
    fn abs_diff_is_symmetric_and_overflow_checked() {
        assert_eq!(abs_diff(3, 10), Ok(7));
        assert_eq!(abs_diff(10, 3), Ok(7));
        assert_eq!(abs_diff(i128::MAX, 0), Ok(i128::MAX));
        assert_eq!(abs_diff(i128::MIN, 0), Err(MathError::Overflow));
        assert_eq!(abs_diff(i128::MAX, i128::MIN), Err(MathError::Overflow));
    }

    #[test]
    fn deviation_bps_rounds_up_and_saturates() {
        assert_eq!(deviation_bps(105, 100), Ok(500));
        assert_eq!(deviation_bps(95, 100), Ok(500));
        assert_eq!(deviation_bps(100, 100), Ok(0));
        // 1/3 of a bp rounds up to 1.
        assert_eq!(deviation_bps(30_001, 30_000), Ok(1));
        assert_eq!(deviation_bps(i128::MAX, 1), Ok(u32::MAX));
        assert_eq!(deviation_bps(i128::MIN, i128::MAX), Ok(u32::MAX));
        assert_eq!(deviation_bps(1, 0), Err(MathError::InvalidReference));
        assert_eq!(deviation_bps(1, -1), Err(MathError::InvalidReference));
    }

    #[test]
    fn within_deviation_is_inclusive() {
        assert_eq!(within_deviation(105, 100, 500), Ok(true));
        assert_eq!(within_deviation(106, 100, 500), Ok(false));
    }

    #[test]
    fn pow10_is_bounded() {
        assert_eq!(pow10(0), Ok(1));
        assert_eq!(pow10(PRICE_DECIMALS), Ok(PRICE_SCALE));
        assert_eq!(pow10(MAX_DECIMALS + 1), Err(MathError::InvalidDecimals));
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
    use std::format;
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
        let mut buffer = values.to_vec();
        median(&mut buffer).unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(CASES))]

        #[test]
        fn is_bounded_by_min_and_max(values in vec(any::<i128>(), 1..=CAP)) {
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
        /// even-count floor-mean moves by exactly c when the sum moves by 2c.
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

        /// Whole i128 domain: always the exact floor mean / middle value,
        /// never a panic — including where a naive `(a + b) / 2` overflows.
        #[test]
        fn never_panics_and_matches_a_wide_reference(values in vec(any::<i128>(), 0..=CAP)) {
            let mut sorted = values.clone();
            sorted.sort();
            let n = sorted.len();
            let expected = if n == 0 {
                Err(MathError::EmptySet)
            } else if n % 2 == 1 {
                Ok(sorted[n / 2])
            } else {
                // Reference via the halved sum in two i128 halves.
                let (a, b) = (sorted[n / 2 - 1], sorted[n / 2]);
                Ok(match a.checked_add(b) {
                    Some(sum) => sum.div_euclid(2),
                    None => floor_mean(a, b),
                })
            };
            let actual = catch_unwind(AssertUnwindSafe(|| {
                let mut v = values.clone();
                median(&mut v)
            }));
            prop_assert_eq!(actual.ok(), Some(expected));
        }

        #[test]
        fn floor_mean_matches_exact_arithmetic(a in any::<i64>(), b in any::<i64>()) {
            let (a, b) = (i128::from(a), i128::from(b));
            prop_assert_eq!(floor_mean(a, b), (a + b).div_euclid(2));
        }

        /// Scaling up then back down (either rounding) is the identity.
        #[test]
        fn rescale_round_trips(v in any::<i64>(), from in 0u32..=9, up in 0u32..=9) {
            let v = i128::from(v);
            let to = from + up;
            let scaled = rescale(v, from, to, Rounding::Floor).unwrap();
            prop_assert_eq!(rescale(scaled, to, from, Rounding::Floor), Ok(v));
            prop_assert_eq!(rescale(scaled, to, from, Rounding::Ceil), Ok(v));
        }

        /// Floor <= exact <= Ceil, and they differ by at most one unit.
        #[test]
        fn rescale_down_brackets_the_exact_value(v in any::<i128>(), down in 1u32..=MAX_DECIMALS) {
            let lo = rescale(v, down, 0, Rounding::Floor).unwrap();
            let hi = rescale(v, down, 0, Rounding::Ceil).unwrap();
            let f = pow10(down).unwrap();
            prop_assert!(hi - lo <= 1);
            prop_assert!(lo.checked_mul(f).map_or(true, |x| x <= v));
            prop_assert!(hi.checked_mul(f).map_or(true, |x| x >= v));
        }

        #[test]
        fn primitives_never_panic(a in any::<i128>(), b in any::<i128>(), d in any::<u32>(), e in any::<u32>()) {
            let r = catch_unwind(|| {
                let _ = rescale(a, d, e, Rounding::Floor);
                let _ = rescale(a, d, e, Rounding::Ceil);
                let _ = abs_diff(a, b);
                let _ = deviation_bps(a, b);
                let _ = check_precision(a, d);
                let _ = pow10(d);
                floor_mean(a, b)
            });
            prop_assert!(r.is_ok());
        }

        /// deviation_bps is the ceil of |v - r| * 10_000 / r.
        #[test]
        fn deviation_bps_is_the_rounded_up_ratio(v in price(), r in price()) {
            let bps = deviation_bps(v, r).unwrap() as i128;
            let diff = (v - r).abs() * 10_000;
            prop_assert!(bps * r >= diff);
            prop_assert!((bps - 1) * r < diff || bps == 0);
        }
    }
}
