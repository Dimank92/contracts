//! Deterministic off-chain Black-Scholes premium fixture generator.
//!
//! This crate lives under `tools/` and is intentionally **not** part of the
//! workspace's contract crates, so it never affects wasm size. It computes
//! European option premiums with the Black-Scholes model and emits stable,
//! diff-friendly fixture vectors that `options_market` tests replay against the
//! on-chain premium/payoff math.
//!
//! # Precision & rounding
//!
//! Premiums are emitted as fixed-point integers at scale [`PREMIUM_SCALE`]
//! (1e6, i.e. six decimal places). Rounding is **half-up** (round half away
//! from zero for positive values, which is all premiums are). This must agree
//! with `options_market/src/math.rs`; a mismatch produces permanent test noise.
//!
//! # Determinism
//!
//! Vectors are committed to the repo rather than recomputed in CI. The
//! generator is deterministic: given the same inputs it always produces the
//! same output bytes (sorted keys, fixed decimals, trailing newline).

use std::fmt::Write as _;

/// Fixed-point scale for premiums: 1e6 (six decimal places).
pub const PREMIUM_SCALE: u64 = 1_000_000;

/// Number of decimal places implied by [`PREMIUM_SCALE`].
pub const PREMIUM_DECIMALS: u32 = 6;

/// Option kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    Call,
    Put,
}

impl OptionKind {
    /// Lowercase string form used in fixtures.
    pub fn as_str(self) -> &'static str {
        match self {
            OptionKind::Call => "call",
            OptionKind::Put => "put",
        }
    }
}

/// Inputs to the Black-Scholes model.
///
/// - `spot`: underlying price `S` (must be > 0)
/// - `strike`: strike price `K` (must be > 0)
/// - `time_to_expiry`: time to expiry `T` in years (must be >= 0)
/// - `volatility`: annualized volatility `sigma` (must be > 0)
/// - `rate`: risk-free rate `r` (may be any finite value)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BsInput {
    pub spot: f64,
    pub strike: f64,
    pub time_to_expiry: f64,
    pub volatility: f64,
    pub rate: f64,
    pub kind: OptionKind,
}

/// Errors produced by the generator for invalid inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BsError {
    /// Spot price must be strictly positive.
    NonPositiveSpot,
    /// Strike price must be strictly positive.
    NonPositiveStrike,
    /// Time to expiry must be non-negative.
    NegativeTime,
    /// Volatility must be strictly positive.
    ZeroVolatility,
    /// An input was NaN or infinite.
    NonFinite,
}

impl std::fmt::Display for BsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BsError::NonPositiveSpot => "spot must be > 0",
            BsError::NonPositiveStrike => "strike must be > 0",
            BsError::NegativeTime => "time_to_expiry must be >= 0",
            BsError::ZeroVolatility => "volatility must be > 0",
            BsError::NonFinite => "inputs must be finite",
        };
        f.write_str(s)
    }
}

impl std::error::Error for BsError {}

/// Standard normal cumulative distribution function.
///
/// Uses the Abramowitz & Stegun 7.1.26 approximation for `erf`, which is
/// accurate to ~1.5e-7 — well within the 1e-6 fixed-point scale we emit.
fn norm_cdf(x: f64) -> f64 {
    // Phi(x) = 0.5 * (1 + erf(x / sqrt(2)))
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}

/// Error function approximation (Abramowitz & Stegun 7.1.26).
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

/// Round a non-negative `f64` to the nearest integer, half-up.
fn round_half_up(x: f64) -> u64 {
    // Premiums are non-negative; add 0.5 and floor.
    (x + 0.5).floor() as u64
}

/// Compute the Black-Scholes premium as a fixed-point integer at
/// [`PREMIUM_SCALE`], rounded half-up.
///
/// Returns `Err` for invalid inputs (see [`BsError`]).
pub fn premium_fixed(input: &BsInput) -> Result<u64, BsError> {
    let premium = premium_f64(input)?;
    // Guard against tiny negative values from floating-point noise.
    let premium = if premium < 0.0 { 0.0 } else { premium };
    Ok(round_half_up(premium * PREMIUM_SCALE as f64))
}

/// Compute the Black-Scholes premium as an `f64` (undiscounted, per unit of
/// underlying). Returns `Err` for invalid inputs.
pub fn premium_f64(input: &BsInput) -> Result<f64, BsError> {
    let BsInput {
        spot,
        strike,
        time_to_expiry,
        volatility,
        rate,
        kind,
    } = *input;

    if !spot.is_finite()
        || !strike.is_finite()
        || !time_to_expiry.is_finite()
        || !volatility.is_finite()
        || !rate.is_finite()
    {
        return Err(BsError::NonFinite);
    }
    if spot <= 0.0 {
        return Err(BsError::NonPositiveSpot);
    }
    if strike <= 0.0 {
        return Err(BsError::NonPositiveStrike);
    }
    if time_to_expiry < 0.0 {
        return Err(BsError::NegativeTime);
    }
    if volatility <= 0.0 {
        return Err(BsError::ZeroVolatility);
    }

    // Degenerate T -> 0 case: premium is the intrinsic value.
    if time_to_expiry == 0.0 {
        let intrinsic = match kind {
            OptionKind::Call => (spot - strike).max(0.0),
            OptionKind::Put => (strike - spot).max(0.0),
        };
        return Ok(intrinsic);
    }

    let sqrt_t = time_to_expiry.sqrt();
    let d1 = ((spot / strike).ln() + (rate + 0.5 * volatility * volatility) * time_to_expiry)
        / (volatility * sqrt_t);
    let d2 = d1 - volatility * sqrt_t;

    let discount = (-rate * time_to_expiry).exp();

    let premium = match kind {
        OptionKind::Call => spot * norm_cdf(d1) - strike * discount * norm_cdf(d2),
        OptionKind::Put => strike * discount * norm_cdf(-d2) - spot * norm_cdf(-d1),
    };

    Ok(premium)
}

/// A single fixture vector: inputs plus the expected fixed-point premium (or
/// expected error).
#[derive(Debug, Clone, PartialEq)]
pub struct Vector {
    pub name: String,
    pub spot: f64,
    pub strike: f64,
    pub time_to_expiry: f64,
    pub volatility: f64,
    pub rate: f64,
    pub kind: OptionKind,
    /// Expected premium at [`PREMIUM_SCALE`], or `None` if an error is expected.
    pub expected_premium: Option<u64>,
    /// Expected error, if any.
    pub expected_error: Option<BsError>,
}

impl Vector {
    fn new(
        name: &str,
        spot: f64,
        strike: f64,
        time_to_expiry: f64,
        volatility: f64,
        rate: f64,
        kind: OptionKind,
    ) -> Self {
        let input = BsInput {
            spot,
            strike,
            time_to_expiry,
            volatility,
            rate,
            kind,
        };
        let (expected_premium, expected_error) = match premium_fixed(&input) {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e)),
        };
        Vector {
            name: name.to_string(),
            spot,
            strike,
            time_to_expiry,
            volatility,
            rate,
            kind,
            expected_premium,
            expected_error,
        }
    }

    fn new_err(
        name: &str,
        spot: f64,
        strike: f64,
        time_to_expiry: f64,
        volatility: f64,
        rate: f64,
        kind: OptionKind,
        err: BsError,
    ) -> Self {
        Vector {
            name: name.to_string(),
            spot,
            strike,
            time_to_expiry,
            volatility,
            rate,
            kind,
            expected_premium: None,
            expected_error: Some(err),
        }
    }
}

/// Format a float with fixed decimals, trimming trailing zeros but keeping at
/// least one decimal place, so output is diff-friendly and stable.
fn fmt_float(x: f64) -> String {
    // Use enough precision to round-trip common fixture values deterministically.
    let mut s = format!("{:.10}", x);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.push('0');
        }
    }
    s
}

/// Format a fixed-point premium as a decimal string at [`PREMIUM_DECIMALS`].
fn fmt_premium(p: u64) -> String {
    let scale = PREMIUM_SCALE as u128;
    let whole = p as u128 / scale;
    let frac = p as u128 % scale;
    format!("{}.{:0width$}", whole, frac, width = PREMIUM_DECIMALS as usize)
}

/// Serialize a set of vectors to the stable, diff-friendly fixture format.
///
/// Format: one vector per block, keys sorted alphabetically, fixed decimals,
/// blank line between blocks, trailing newline at end of file.
pub fn serialize(vectors: &[Vector]) -> String {
    let mut out = String::new();
    for (i, v) in vectors.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // Keys sorted alphabetically: expected_error, expected_premium, kind,
        // name, rate, spot, strike, time_to_expiry, volatility.
        let err = match &v.expected_error {
            Some(e) => e.to_string(),
            None => "none".to_string(),
        };
        let prem = match v.expected_premium {
            Some(p) => fmt_premium(p),
            None => "none".to_string(),
        };
        writeln!(out, "expected_error = {}", err).unwrap();
        writeln!(out, "expected_premium = {}", prem).unwrap();
        writeln!(out, "kind = {}", v.kind.as_str()).unwrap();
        writeln!(out, "name = {}", v.name).unwrap();
        writeln!(out, "rate = {}", fmt_float(v.rate)).unwrap();
        writeln!(out, "spot = {}", fmt_float(v.spot)).unwrap();
        writeln!(out, "strike = {}", fmt_float(v.strike)).unwrap();
        writeln!(out, "time_to_expiry = {}", fmt_float(v.time_to_expiry)).unwrap();
        writeln!(out, "volatility = {}", fmt_float(v.volatility)).unwrap();
    }
    out
}

/// Build the canonical fixture vector set.
///
/// Covers at-the-money, deep in/out-of-the-money, very short and long
/// expiries, high and low volatility, the degenerate `T -> 0` case, and
/// invalid inputs (negative time, zero vol) with explicit expected errors.
pub fn canonical_vectors() -> Vec<Vector> {
    use OptionKind::{Call, Put};
    vec![
        // At-the-money.
        Vector::new("atm_call", 100.0, 100.0, 1.0, 0.2, 0.05, Call),
        Vector::new("atm_put", 100.0, 100.0, 1.0, 0.2, 0.05, Put),
        // Deep in-the-money.
        Vector::new("deep_itm_call", 200.0, 100.0, 1.0, 0.2, 0.05, Call),
        Vector::new("deep_itm_put", 50.0, 100.0, 1.0, 0.2, 0.05, Put),
        // Deep out-of-the-money.
        Vector::new("deep_otm_call", 50.0, 100.0, 1.0, 0.2, 0.05, Call),
        Vector::new("deep_otm_put", 200.0, 100.0, 1.0, 0.2, 0.05, Put),
        // Very short expiry.
        Vector::new("short_expiry_call", 100.0, 100.0, 1.0 / 365.0, 0.2, 0.05, Call),
        Vector::new("short_expiry_put", 100.0, 100.0, 1.0 / 365.0, 0.2, 0.05, Put),
        // Long expiry.
        Vector::new("long_expiry_call", 100.0, 100.0, 5.0, 0.2, 0.05, Call),
        Vector::new("long_expiry_put", 100.0, 100.0, 5.0, 0.2, 0.05, Put),
        // High volatility.
        Vector::new("high_vol_call", 100.0, 100.0, 1.0, 1.0, 0.05, Call),
        Vector::new("high_vol_put", 100.0, 100.0, 1.0, 1.0, 0.05, Put),
        // Low volatility.
        Vector::new("low_vol_call", 100.0, 100.0, 1.0, 0.01, 0.05, Call),
        Vector::new("low_vol_put", 100.0, 100.0, 1.0, 0.01, 0.05, Put),
        // Degenerate T -> 0 (intrinsic value).
        Vector::new("zero_time_itm_call", 120.0, 100.0, 0.0, 0.2, 0.05, Call),
        Vector::new("zero_time_otm_call", 80.0, 100.0, 0.0, 0.2, 0.05, Call),
        Vector::new("zero_time_itm_put", 80.0, 100.0, 0.0, 0.2, 0.05, Put),
        Vector::new("zero_time_otm_put", 120.0, 100.0, 0.0, 0.2, 0.05, Put),
        // Invalid inputs with explicit expected errors.
        Vector::new_err(
            "invalid_negative_time",
            100.0,
            100.0,
            -1.0,
            0.2,
            0.05,
            Call,
            BsError::NegativeTime,
        ),
        Vector::new_err(
            "invalid_zero_vol",
            100.0,
            100.0,
            1.0,
            0.0,
            0.05,
            Call,
            BsError::ZeroVolatility,
        ),
        Vector::new_err(
            "invalid_nonpositive_spot",
            0.0,
            100.0,
            1.0,
            0.2,
            0.05,
            Call,
            BsError::NonPositiveSpot,
        ),
        Vector::new_err(
            "invalid_nonpositive_strike",
            100.0,
            0.0,
            1.0,
            0.2,
            0.05,
            Call,
            BsError::NonPositiveStrike,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn norm_cdf_known_values() {
        assert!(approx(norm_cdf(0.0), 0.5, 1e-6));
        assert!(approx(norm_cdf(1.0), 0.841_344_746, 1e-6));
        assert!(approx(norm_cdf(-1.0), 0.158_655_254, 1e-6));
        assert!(approx(norm_cdf(1.96), 0.975_002_105, 1e-5));
    }

    #[test]
    fn atm_call_matches_reference() {
        // Reference: S=100, K=100, T=1, sigma=0.2, r=0.05 -> ~10.4506
        let input = BsInput {
            spot: 100.0,
            strike: 100.0,
            time_to_expiry: 1.0,
            volatility: 0.2,
            rate: 0.05,
            kind: OptionKind::Call,
        };
        let p = premium_f64(&input).unwrap();
        assert!(approx(p, 10.450_584, 1e-3), "got {}", p);
    }

    #[test]
    fn atm_put_matches_reference() {
        // Reference: S=100, K=100, T=1, sigma=0.2, r=0.05 -> ~5.5735
        let input = BsInput {
            spot: 100.0,
            strike: 100.0,
            time_to_expiry: 1.0,
            volatility: 0.2,
            rate: 0.05,
            kind: OptionKind::Put,
        };
        let p = premium_f64(&input).unwrap();
        assert!(approx(p, 5.573_526, 1e-3), "got {}", p);
    }

    #[test]
    fn put_call_parity() {
        // C - P = S - K * e^{-rT}
        let base = BsInput {
            spot: 100.0,
            strike: 100.0,
            time_to_expiry: 1.0,
            volatility: 0.2,
            rate: 0.05,
            kind: OptionKind::Call,
        };
        let call = premium_f64(&base).unwrap();
        let put = premium_f64(&BsInput {
            kind: OptionKind::Put,
            ..base
        })
        .unwrap();
        let expected = base.spot - base.strike * (-base.rate * base.time_to_expiry).exp();
        assert!(approx(call - put, expected, 1e-6));
    }

    #[test]
    fn zero_time_is_intrinsic() {
        let itm_call = BsInput {
            spot: 120.0,
            strike: 100.0,
            time_to_expiry: 0.0,
            volatility: 0.2,
            rate: 0.05,
            kind: OptionKind::Call,
        };
        assert_eq!(premium_fixed(&itm_call).unwrap(), 20 * PREMIUM_SCALE);

        let otm_call = BsInput {
            spot: 80.0,
            strike: 100.0,
            time_to_expiry: 0.0,
            volatility: 0.2,
            rate: 0.05,
            kind: OptionKind::Call,
        };
        assert_eq!(premium_fixed(&otm_call).unwrap(), 0);

        let itm_put = BsInput {
            spot: 80.0,
            strike: 100.0,
            time_to_expiry: 0.0,
            volatility: 0.2,
            rate: 0.05,
            kind: OptionKind::Put,
        };
        assert_eq!(premium_fixed(&itm_put).unwrap(), 20 * PREMIUM_SCALE);
    }

    #[test]
    fn invalid_inputs_error() {
        let neg_time = BsInput {
            spot: 100.0,
            strike: 100.0,
            time_to_expiry: -1.0,
            volatility: 0.2,
            rate: 0.05,
            kind: OptionKind::Call,
        };
        assert_eq!(premium_fixed(&neg_time), Err(BsError::NegativeTime));

        let zero_vol = BsInput {
            spot: 100.0,
            strike: 100.0,
            time_to_expiry: 1.0,
            volatility: 0.0,
            rate: 0.05,
            kind: OptionKind::Call,
        };
        assert_eq!(premium_fixed(&zero_vol), Err(BsError::ZeroVolatility));

        let zero_spot = BsInput {
            spot: 0.0,
            strike: 100.0,
            time_to_expiry: 1.0,
            volatility: 0.2,
            rate: 0.05,
            kind: OptionKind::Call,
        };
        assert_eq!(premium_fixed(&zero_spot), Err(BsError::NonPositiveSpot));

        let zero_strike = BsInput {
            spot: 100.0,
            strike: 0.0,
            time_to_expiry: 1.0,
            volatility: 0.2,
            rate: 0.05,
            kind: OptionKind::Call,
        };
        assert_eq!(premium_fixed(&zero_strike), Err(BsError::NonPositiveStrike));
    }

    #[test]
    fn rounding_is_half_up() {
        // 1.0000005 * 1e6 = 1000000.5 -> rounds to 1000001
        assert_eq!(round_half_up(1.000_000_5 * PREMIUM_SCALE as f64), 1_000_001);
        // 1.0000004 * 1e6 = 1000000.4 -> rounds to 1000000
        assert_eq!(round_half_up(1.000_000_4 * PREMIUM_SCALE as f64), 1_000_000);
    }

    #[test]
    fn fmt_premium_fixed_decimals() {
        assert_eq!(fmt_premium(0), "0.000000");
        assert_eq!(fmt_premium(1), "0.000001");
        assert_eq!(fmt_premium(1_234_567), "1.234567");
        assert_eq!(fmt_premium(20 * PREMIUM_SCALE), "20.000000");
    }

    #[test]
    fn serialize_is_deterministic() {
        let vectors = canonical_vectors();
        let a = serialize(&vectors);
        let b = serialize(&vectors);
        assert_eq!(a, b);
        // Keys are sorted alphabetically within each block.
        let first_block: Vec<&str> = a.lines().take(9).collect();
        assert!(first_block[0].starts_with("expected_error ="));
        assert!(first_block[1].starts_with("expected_premium ="));
        assert!(first_block[2].starts_with("kind ="));
        assert!(first_block[3].starts_with("name ="));
        assert!(first_block[4].starts_with("rate ="));
        assert!(first_block[5].starts_with("spot ="));
        assert!(first_block[6].starts_with("strike ="));
        assert!(first_block[7].starts_with("time_to_expiry ="));
        assert!(first_block[8].starts_with("volatility ="));
    }

    #[test]
    fn canonical_vectors_cover_required_cases() {
        let vectors = canonical_vectors();
        let names: Vec<&str> = vectors.iter().map(|v| v.name.as_str()).collect();
        for required in [
            "atm_call",
            "atm_put",
            "deep_itm_call",
            "deep_itm_put",
            "deep_otm_call",
            "deep_otm_put",
            "short_expiry_call",
            "short_expiry_put",
            "long_expiry_call",
            "long_expiry_put",
            "high_vol_call",
            "high_vol_put",
            "low_vol_call",
            "low_vol_put",
            "zero_time_itm_call",
            "zero_time_otm_call",
            "zero_time_itm_put",
            "zero_time_otm_put",
            "invalid_negative_time",
            "invalid_zero_vol",
        ] {
            assert!(names.contains(&required), "missing vector {}", required);
        }
    }

    #[test]
    fn canonical_vectors_are_consistent_with_model() {
        for v in canonical_vectors() {
            let input = BsInput {
                spot: v.spot,
                strike: v.strike,
                time_to_expiry: v.time_to_expiry,
                volatility: v.volatility,
                rate: v.rate,
                kind: v.kind,
            };
            match premium_fixed(&input) {
                Ok(p) => {
                    assert_eq!(Some(p), v.expected_premium, "vector {}", v.name);
                    assert!(v.expected_error.is_none(), "vector {}", v.name);
                }
                Err(e) => {
                    assert_eq!(Some(e), v.expected_error, "vector {}", v.name);
                    assert!(v.expected_premium.is_none(), "vector {}", v.name);
                }
            }
        }
    }
}