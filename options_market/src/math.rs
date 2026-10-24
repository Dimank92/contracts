use crate::types::OptionType;

pub const PRICE_PRECISION: i128 = 10_000_000; // 1e7
pub const RATE_PRECISION: i128 = 1_000_000_000; // 1e9
pub const MIN_COLLATERAL_RATIO: i128 = 1_100_000_000; // 110% over-collateralization for puts
pub const SETTLEMENT_WINDOW: u64 = 86_400; // 24h soft window after expiry to exercise
pub const FORFEITURE_WINDOW: u64 = 90 * 86_400; // 90d window before unexercised ITM long sweeps to treasury

pub const DEFAULT_FEE_RATE_BPS: i128 = 50; // 0.5%
pub const MAX_FEE_RATE_BPS: i128 = 1_000; // 10% hard ceiling, even for the admin

/// Caps how many series can ever be listed for a given underlying, so a
/// single symbol can't accumulate unbounded storage entries over the
/// contract's lifetime. This counts every series ever created, not
/// currently-active ones — cancelling or letting a series expire doesn't
/// free up room, since nothing about storage usage shrinks when that
/// happens either.
pub const MAX_SERIES_PER_UNDERLYING: u32 = 50;

/// Caps how many position_ids exercise_batch/reclaim_batch will process in
/// a single call, so a caller with a very large position count can't build
/// a batch that blows through Soroban's per-call resource limits.
pub const MAX_BATCH_SIZE: u32 = 25;

/// Most series one `settle_expiry` call settles. Sized against the
/// per-transaction CPU / ledger-I/O budget: each series is one persistent
/// read and two writes (see `settle_expiry_full_batch_fits_the_budget`).
pub const MAX_EXPIRY_BATCH: u32 = 25;

/// Series ids per expiry-calendar page.
pub const EXPIRY_PAGE_SIZE: u32 = 25;

/// Protocol fee on an amount, given a rate in basis points (1 bps = 0.01%).
pub fn calc_fee(amount: i128, fee_rate_bps: i128) -> i128 {
    amount
        .checked_mul(fee_rate_bps)
        .unwrap()
        .checked_div(10_000)
        .unwrap()
}

/// Cash payout at settlement:
/// Call: max(0, settlement - strike) × contracts / PRICE_PRECISION
/// Put:  max(0, strike - settlement) × contracts / PRICE_PRECISION
pub fn calc_payout(
    option_type: &OptionType,
    strike: i128,
    settlement: i128,
    contracts: i128,
) -> i128 {
    let intrinsic = match option_type {
        OptionType::Call => (settlement - strike).max(0),
        OptionType::Put => (strike - settlement).max(0),
    };
    contracts
        .checked_mul(intrinsic)
        .unwrap()
        .checked_div(PRICE_PRECISION)
        .unwrap()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn calc_fee_computes_bps_of_amount() {
        assert_eq!(calc_fee(40_000_000, 50), 200_000); // 0.5% of 40M
        assert_eq!(calc_fee(40_000_000, 100), 400_000); // 1% of 40M
    }

    #[test]
    fn calc_fee_of_zero_bps_is_zero() {
        assert_eq!(calc_fee(40_000_000, 0), 0);
    }

    #[test]
    fn calc_fee_truncates_towards_zero_on_a_non_exact_division() {
        // 999 * 50 / 10_000 = 4.995, integer division truncates to 4 —
        // callers must not assume this rounds to the nearest bps.
        assert_eq!(calc_fee(999, 50), 4);
    }

    #[test]
    fn calc_payout_call_is_zero_at_the_money() {
        assert_eq!(
            calc_payout(&OptionType::Call, 700_000_000, 700_000_000, PRICE_PRECISION),
            0
        );
    }

    #[test]
    fn calc_payout_call_is_zero_out_of_the_money() {
        assert_eq!(
            calc_payout(&OptionType::Call, 700_000_000, 650_000_000, PRICE_PRECISION),
            0
        );
    }

    #[test]
    fn calc_payout_call_pays_the_intrinsic_value_in_the_money() {
        // Strike 700, settlement 750 -> 50 of intrinsic value, 1 contract.
        assert_eq!(
            calc_payout(&OptionType::Call, 700_000_000, 750_000_000, PRICE_PRECISION),
            50_000_000
        );
    }

    #[test]
    fn calc_payout_put_is_zero_out_of_the_money() {
        assert_eq!(
            calc_payout(&OptionType::Put, 700_000_000, 750_000_000, PRICE_PRECISION),
            0
        );
    }

    #[test]
    fn calc_payout_put_pays_the_intrinsic_value_in_the_money() {
        // Strike 700, settlement 650 -> 50 of intrinsic value, 1 contract.
        assert_eq!(
            calc_payout(&OptionType::Put, 700_000_000, 650_000_000, PRICE_PRECISION),
            50_000_000
        );
    }

    #[test]
    fn calc_payout_scales_linearly_with_contracts() {
        let one_contract =
            calc_payout(&OptionType::Call, 700_000_000, 750_000_000, PRICE_PRECISION);
        let three_contracts = calc_payout(
            &OptionType::Call,
            700_000_000,
            750_000_000,
            3 * PRICE_PRECISION,
        );
        assert_eq!(three_contracts, one_contract * 3);
    }
}
