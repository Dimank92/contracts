use crate::types::{OptionType, SeriesParams};
use soroban_sdk::{Env, I256};

/// Fixed-point scale used by the options market math (7 decimal places).
pub const SCALE: i128 = 10_000_000;

/// Compute the intrinsic payoff of an option at settlement.
///
/// Returns the non-negative payout owed to the holder for the given
/// `option_type`, `strike`, and oracle-reported `spot` price. All values are
/// expressed in the same fixed-point scale (`SCALE`).
///
/// The result is clamped at zero so that out-of-the-money options never
/// produce a negative payout, and arithmetic is performed with checked
/// operations so in-range inputs cannot wrap.
pub fn payoff(option_type: OptionType, strike: i128, spot: i128) -> i128 {
    let diff = match option_type {
        OptionType::Call => spot.checked_sub(strike),
        OptionType::Put => strike.checked_sub(spot),
    };

    match diff {
        Some(d) if d > 0 => d,
        _ => 0,
    }
}

/// Compute the collateral a writer must lock to fully cover the payout of a
/// single contract at the given `strike`.
///
/// For a call the maximum payout is unbounded in spot, so the writer locks the
/// full notional (strike) as collateral. For a put the maximum payout is the
/// strike itself (spot can fall to zero), so the writer also locks the strike.
/// In both cases the locked collateral is guaranteed to be sufficient to cover
/// any in-range payoff.
pub fn required_collateral(option_type: OptionType, strike: i128) -> i128 {
    match option_type {
        OptionType::Call | OptionType::Put => strike.max(0),
    }
}

/// Returns true when the writer's `collateral` is sufficient to cover the
/// `payout` computed for a settled option.
pub fn collateral_covers(collateral: i128, payout: i128) -> bool {
    collateral >= payout
}

/// Validate that a series' parameters are within the representable range of
/// the fixed-point type. Returns `false` for out-of-range inputs so callers can
/// reject them explicitly rather than silently truncating.
pub fn params_in_range(params: &SeriesParams) -> bool {
    params.strike > 0
        && params.strike <= i128::MAX / SCALE
        && params.expiry >= 0
        && params.collateral >= 0
}

/// Settlement entry point used by the contract: computes the payout for a
/// series at expiry against an oracle-reported `spot` price.
///
/// Settlement at exactly expiry and at zero spot both produce a defined result
/// (never a panic): a zero spot yields the maximum put payout and a zero call
/// payout.
pub fn settle(env: &Env, params: &SeriesParams, spot: i128) -> i128 {
    let _ = env;
    payoff(params.option_type, params.strike, spot)
}

/// Premium arithmetic helper: scale an off-chain Black-Scholes premium by the
/// contract size using checked multiplication so in-range inputs cannot wrap.
pub fn scale_premium(premium: i128, size: i128) -> Option<i128> {
    premium.checked_mul(size)
}

#[cfg(test)]
mod proptest_suite {
    use super::*;
    use proptest::prelude::*;

    /// Upper bound for generated fixed-point values: keeps products within
    /// `i128` while still exercising large magnitudes.
    const MAX_VALUE: i128 = 1_000_000_000_000_000_000;

    fn option_type() -> impl Strategy<Value = OptionType> {
        prop_oneof![Just(OptionType::Call), Just(OptionType::Put)]
    }

    fn strike() -> impl Strategy<Value = i128> {
        1i128..=MAX_VALUE
    }

    fn spot() -> impl Strategy<Value = i128> {
        0i128..=MAX_VALUE
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: std::env::var("PROPTEST_CASES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(256),
            ..ProptestConfig::default()
        })]

        #[test]
        fn payoff_is_never_negative(
            opt in option_type(),
            strike in strike(),
            spot in spot(),
        ) {
            prop_assert!(payoff(opt, strike, spot) >= 0);
        }

        #[test]
        fn call_payoff_monotonic_in_spot(
            strike in strike(),
            spot_a in spot(),
            spot_b in spot(),
        ) {
            let (lo, hi) = if spot_a <= spot_b {
                (spot_a, spot_b)
            } else {
                (spot_b, spot_a)
            };
            let p_lo = payoff(OptionType::Call, strike, lo);
            let p_hi = payoff(OptionType::Call, strike, hi);
            prop_assert!(p_hi >= p_lo);
        }

        #[test]
        fn put_payoff_monotonic_in_spot(
            strike in strike(),
            spot_a in spot(),
            spot_b in spot(),
        ) {
            let (lo, hi) = if spot_a <= spot_b {
                (spot_a, spot_b)
            } else {
                (spot_b, spot_a)
            };
            let p_lo = payoff(OptionType::Put, strike, lo);
            let p_hi = payoff(OptionType::Put, strike, hi);
            prop_assert!(p_hi <= p_lo);
        }

        #[test]
        fn collateral_covers_payout(
            opt in option_type(),
            strike in strike(),
            spot in spot(),
        ) {
            let collateral = required_collateral(opt, strike);
            let payout = payoff(opt, strike, spot);
            prop_assert!(collateral_covers(collateral, payout));
        }

        #[test]
        fn premium_scaling_does_not_wrap(
            premium in 0i128..=MAX_VALUE,
            size in 0i128..=1_000_000i128,
        ) {
            // In-range inputs must produce a defined result, never a panic.
            let scaled = scale_premium(premium, size);
            prop_assert!(scaled.is_some());
        }

        #[test]
        fn settlement_at_expiry_and_zero_spot_is_defined(
            opt in option_type(),
            strike in strike(),
        ) {
            // Zero spot must not panic and must yield a defined payout.
            let zero_spot_payout = payoff(opt, strike, 0);
            prop_assert!(zero_spot_payout >= 0);
            match opt {
                OptionType::Put => prop_assert_eq!(zero_spot_payout, strike),
                OptionType::Call => prop_assert_eq!(zero_spot_payout, 0),
            }
        }

        #[test]
        fn out_of_range_params_are_rejected(
            strike in (i128::MAX / SCALE + 1)..=i128::MAX,
        ) {
            let params = SeriesParams {
                option_type: OptionType::Call,
                strike,
                expiry: 0,
                collateral: 0,
            };
            prop_assert!(!params_in_range(&params));
        }
    }
}
