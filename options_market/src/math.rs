//! Fixed-point collateral math for the `options_market` contract.
//!
//! All collateral arithmetic lives here so the entrypoints and storage layers
//! stay thin. Every function is total: adversarial inputs return `None`
//! instead of panicking, and callers translate that into a contract error.

use types::{OptionType, Series};

/// Compute the collateral a writer must lock to mint `amount` contracts of
/// `series`.
///
/// * Call series: the writer locks `amount` units of the underlying token.
/// * Put series: the writer locks `strike * amount` units of the quote token.
///
/// Returns `None` for non-positive amounts or when the multiplication would
/// overflow `i128`.
pub fn collateral_for(series: &Series, amount: i128) -> Option<i128> {
    if amount <= 0 {
        return None;
    }

    match series.option_type {
        OptionType::Call => Some(amount),
        OptionType::Put => amount.checked_mul(series.strike),
    }
}

/// Add two collateral amounts, returning `None` on overflow.
pub fn add_collateral(a: i128, b: i128) -> Option<i128> {
    a.checked_add(b)
}

/// Subtract a released collateral amount, returning `None` on underflow.
pub fn sub_collateral(a: i128, b: i128) -> Option<i128> {
    a.checked_sub(b)
}
