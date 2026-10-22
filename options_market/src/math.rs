//! Math helpers for the options market: pricing, collateral, and settlement math.

use soroban_sdk::{contracttype, Env};

use crate::storage::Position;

/// Basis points denominator (100% = 10_000 bps).
pub const BPS_DENOMINATOR: i128 = 10_000;

/// Rounding mode used when computing the buy-back premium on unwind.
///
/// The buy-back premium is the amount the writer must pay to close the short
/// early. We always round *up* (in favor of the buyer / protocol) so that the
/// writer can never release more collateral than the short is worth. This keeps
/// the `vault` tag ledger from drifting below the sum of live positions'
/// collateral due to rounding.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoundingMode {
    /// Round the result up to the next whole unit (ceil).
    Up,
    /// Round the result down to the previous whole unit (floor).
    Down,
}

/// Result of a buy-back premium computation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuyBackQuote {
    /// Premium owed to buy back the short at the current oracle price.
    pub premium: i128,
    /// Collateral that would be released to the writer (collateral - premium).
    /// Guaranteed to be `>= 0` when `ok` is true.
    pub collateral_release: i128,
    /// Whether the quote is valid (premium does not exceed collateral).
    pub ok: bool,
}

/// Compute the current premium for a short position at the current oracle price.
///
/// The premium is the intrinsic value of the remaining short contracts at the
/// current oracle price, scaled by the position's premium rate and the fraction
/// of the term remaining. This mirrors the settlement math applied at expiry,
/// but evaluated at the current oracle price before expiry.
///
/// # Arguments
/// * `env` - Contract environment (for ledger timestamp).
/// * `position` - The writer's position being unwound.
/// * `oracle_price` - Current oracle price for the underlying.
/// * `now` - Current ledger timestamp.
///
/// # Rounding
/// The premium is rounded *up* (see [`RoundingMode::Up`]) so the writer can
/// never release more collateral than the short is worth.
///
/// # Returns
/// A [`BuyBackQuote`] with the premium, the resulting collateral release, and
/// an `ok` flag. When `ok` is false the premium exceeds the recorded collateral
/// and the unwind must be rejected by the caller.
pub fn buy_back_premium(
    env: &Env,
    position: &Position,
    oracle_price: i128,
    now: u64,
) -> BuyBackQuote {
    let _ = env;

    // No remaining contracts -> nothing to buy back.
    if position.amount <= 0 {
        return BuyBackQuote {
            premium: 0,
            collateral_release: position.collateral,
            ok: true,
        };
    }

    // Compute intrinsic value of the short at the current oracle price.
    // For a call: max(oracle - strike, 0); for a put: max(strike - oracle, 0).
    let intrinsic = if position.is_call {
        if oracle_price > position.strike {
            oracle_price - position.strike
        } else {
            0
        }
    } else if position.strike > oracle_price {
        position.strike - oracle_price
    } else {
        0
    };

    // Scale intrinsic value by the number of contracts.
    // Use checked math; on overflow treat as invalid (premium exceeds collateral).
    let gross = match intrinsic.checked_mul(position.amount) {
        Some(v) => v,
        None => {
            return BuyBackQuote {
                premium: i128::MAX,
                collateral_release: 0,
                ok: false,
            }
        }
    };

    // Apply the position's premium rate (in bps) if present, rounding up.
    let with_rate = match position.premium_bps {
        Some(bps) if bps > 0 => {
            let scaled = match gross.checked_mul(bps as i128) {
                Some(v) => v,
                None => {
                    return BuyBackQuote {
                        premium: i128::MAX,
                        collateral_release: 0,
                        ok: false,
                    }
                }
            };
            div_round(scaled, BPS_DENOMINATOR, RoundingMode::Up)
        }
        _ => gross,
    };

    // Scale by the fraction of the term remaining, if the position has a term.
    // remaining_fraction = (expiry - now) / (expiry - start), rounded up so the
    // writer pays at least the pro-rata value of the remaining term.
    let premium = if position.expiry > position.start && position.expiry > now {
        let remaining = (position.expiry - now) as i128;
        let total = (position.expiry - position.start) as i128;
        let scaled = match with_rate.checked_mul(remaining) {
            Some(v) => v,
            None => {
                return BuyBackQuote {
                    premium: i128::MAX,
                    collateral_release: 0,
                    ok: false,
                }
            }
        };
        div_round(scaled, total, RoundingMode::Up)
    } else if position.expiry <= now {
        // At or past expiry the full premium is due.
        with_rate
    } else {
        // No term information: charge the full premium.
        with_rate
    };

    // Guard: the collateral release cannot go negative.
    let collateral_release = position.collateral.saturating_sub(premium);
    let ok = premium <= position.collateral;

    BuyBackQuote {
        premium,
        collateral_release,
        ok,
    }
}

/// Integer division with an explicit rounding mode.
///
/// `denominator` must be positive; callers guarantee this. For `RoundingMode::Up`
/// the result is rounded away from zero for positive numerators (ceil), which is
/// the conservative direction for premiums owed by the writer.
pub fn div_round(numerator: i128, denominator: i128, mode: RoundingMode) -> i128 {
    debug_assert!(denominator > 0, "denominator must be positive");

    let quotient = numerator / denominator;
    let remainder = numerator % denominator;

    match mode {
        RoundingMode::Down => quotient,
        RoundingMode::Up => {
            if remainder != 0 && numerator > 0 {
                quotient + 1
            } else {
                quotient
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    fn make_position(
        amount: i128,
        strike: i128,
        collateral: i128,
        is_call: bool,
        premium_bps: Option<u32>,
        start: u64,
        expiry: u64,
    ) -> Position {
        Position {
            writer: soroban_sdk::Address::from_str(
                &Env::default(),
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            ),
            amount,
            strike,
            collateral,
            is_call,
            premium_bps,
            start,
            expiry,
            settled: false,
        }
    }

    #[test]
    fn div_round_up_and_down() {
        assert_eq!(div_round(10, 3, RoundingMode::Up), 4);
        assert_eq!(div_round(10, 3, RoundingMode::Down), 3);
        assert_eq!(div_round(9, 3, RoundingMode::Up), 3);
        assert_eq!(div_round(0, 3, RoundingMode::Up), 0);
    }

    #[test]
    fn buy_back_zero_amount() {
        let env = Env::default();
        let pos = make_position(0, 100, 500, true, None, 0, 100);
        let q = buy_back_premium(&env, &pos, 150, 50);
        assert_eq!(q.premium, 0);
        assert_eq!(q.collateral_release, 500);
        assert!(q.ok);
    }

    #[test]
    fn buy_back_call_itm() {
        let env = Env::default();
        // 10 contracts, strike 100, oracle 150 -> intrinsic 50 * 10 = 500.
        let pos = make_position(10, 100, 1000, true, None, 0, 100);
        let q = buy_back_premium(&env, &pos, 150, 50);
        // remaining fraction = 50/100 -> 500 * 50 / 100 = 250.
        assert_eq!(q.premium, 250);
        assert_eq!(q.collateral_release, 750);
        assert!(q.ok);
    }

    #[test]
    fn buy_back_put_itm() {
        let env = Env::default();
        // 10 contracts, strike 100, oracle 50 -> intrinsic 50 * 10 = 500.
        let pos = make_position(10, 100, 1000, false, None, 0, 100);
        let q = buy_back_premium(&env, &pos, 50, 50);
        assert_eq!(q.premium, 250);
        assert_eq!(q.collateral_release, 750);
        assert!(q.ok);
    }

    #[test]
    fn buy_back_otm_zero_premium() {
        let env = Env::default();
        let pos = make_position(10, 100, 1000, true, None, 0, 100);
        let q = buy_back_premium(&env, &pos, 50, 50);
        assert_eq!(q.premium, 0);
        assert_eq!(q.collateral_release, 1000);
        assert!(q.ok);
    }

    #[test]
    fn buy_back_exceeds_collateral_not_ok() {
        let env = Env::default();
        // Deep ITM: intrinsic 100 * 10 = 1000, remaining fraction 1 -> 1000.
        let pos = make_position(10, 100, 500, true, None, 0, 100);
        let q = buy_back_premium(&env, &pos, 200, 100);
        assert_eq!(q.premium, 1000);
        assert_eq!(q.collateral_release, 0);
        assert!(!q.ok);
    }

    #[test]
    fn buy_back_rounds_up() {
        let env = Env::default();
        // intrinsic 1 * 3 = 3, remaining 1/3 -> 3 * 1 / 3 = 1 exactly.
        // Use a case that requires rounding: intrinsic 1 * 1 = 1, remaining 1/3.
        let pos = make_position(1, 100, 100, true, None, 0, 3);
        let q = buy_back_premium(&env, &pos, 101, 2);
        // remaining = 1, total = 3 -> 1 * 1 / 3 = 0.33 -> rounds up to 1.
        assert_eq!(q.premium, 1);
        assert_eq!(q.collateral_release, 99);
        assert!(q.ok);
    }
}