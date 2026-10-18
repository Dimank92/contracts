//! Adversarial price oracle contracts.
//!
//! Each adversary here exposes `get_price(symbol) -> Option<i128>` — the
//! only function `options_market::set_settlement_price_from_oracle` ever
//! calls on an oracle. The rest of the price_oracle interface (`add_feeder`,
//! `report_price`, etc.) is included as no-ops so the same address can be
//! used anywhere a `price_oracle` contract is expected.
//!
//! ## Adversaries
//!
//! | Struct                    | Behaviour                                                  |
//! |---------------------------|------------------------------------------------------------|
//! | [`AlwaysNoneOracle`]      | `get_price()` always returns `None` — no price ever available. |
//! | [`ExtremeHighOracle`]     | `get_price()` returns `i128::MAX` — maximum price.         |
//! | [`ExtremeLowOracle`]      | `get_price()` returns `1` — minimum non-zero price.        |
//! | [`RevertingOracle`]       | `get_price()` always panics — the oracle contract itself is broken. |
//! | [`StaleOracle`]           | Reports a price but marks it as extremely old; any staleness check should reject it. |
//! | [`NegativePriceOracle`]   | `get_price()` returns `-1` — invalid price for settlement. |

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, panic_with_error, Address, Env, Symbol};

// ─── shared storage ──────────────────────────────────────────────────────────

#[contracttype]
enum OracleKey {
    Admin,
    Price(Symbol),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum OracleError {
    OracleReverted = 1,
    InvalidPrice = 2,
}

// ─── AlwaysNoneOracle ────────────────────────────────────────────────────────

/// An oracle that returns `None` for every `get_price` call, regardless
/// of how many feeders have reported. Simulates a situation where quorum
/// has never been reached or all reports have gone stale.
///
/// **Vulnerability exercised (issue #115):** `set_settlement_price_from_oracle`
/// must handle `None` from the oracle gracefully — i.e. not interpret a
/// missing price as zero and settle at the wrong value. Tests confirm it
/// propagates an error / reverts instead.
mod always_none_oracle_impl {
    use super::*;

    #[contract]
    pub struct AlwaysNoneOracle;

    #[contractimpl]
    impl AlwaysNoneOracle {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            env.storage().instance().set(&OracleKey::Admin, &admin);
        }

        /// Always returns `None`.
        pub fn get_price(_env: Env, _symbol: Symbol) -> Option<i128> {
            None
        }

        // ── no-op stubs for the rest of the price_oracle interface ──
        pub fn add_feeder(_env: Env, _feeder: Address) {}
        pub fn remove_feeder(_env: Env, _feeder: Address) {}
        pub fn report_price(_env: Env, _feeder: Address, _symbol: Symbol, _price: i128) {}
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool { false }
        pub fn get_feeder_count(_env: Env) -> u32 { 0 }
        pub fn get_max_staleness(_env: Env) -> u64 { 3600 }
        pub fn get_min_reports(_env: Env) -> u32 { 1 }
        pub fn is_paused(_env: Env) -> bool { false }
        pub fn get_latest_report(_env: Env, _symbol: Symbol, _feeder: Address) -> Option<(i128, u64)> { None }
    }
}
pub use always_none_oracle_impl::{AlwaysNoneOracle, AlwaysNoneOracleClient};

// ─── ExtremeHighOracle ───────────────────────────────────────────────────────

/// An oracle that returns `i128::MAX` as the settlement price for every
/// symbol. Models a compromised or buggy feeder reporting an impossibly
/// high price.
///
/// **Vulnerability exercised (issue #115):** Settlement payout arithmetic
/// (strike_price × contracts / PRICE_PRECISION) performed on `i128::MAX`
/// will overflow unless checked. Confirms overflow-safe arithmetic in
/// options_market's exercise path.
mod extreme_high_oracle_impl {
    use super::*;

    #[contract]
    pub struct ExtremeHighOracle;

    #[contractimpl]
    impl ExtremeHighOracle {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            env.storage().instance().set(&OracleKey::Admin, &admin);
        }

        /// Always returns `i128::MAX`.
        pub fn get_price(_env: Env, _symbol: Symbol) -> Option<i128> {
            Some(i128::MAX)
        }

        pub fn add_feeder(_env: Env, _feeder: Address) {}
        pub fn remove_feeder(_env: Env, _feeder: Address) {}
        pub fn report_price(_env: Env, _feeder: Address, _symbol: Symbol, _price: i128) {}
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool { false }
        pub fn get_feeder_count(_env: Env) -> u32 { 1 }
        pub fn get_max_staleness(_env: Env) -> u64 { 3600 }
        pub fn get_min_reports(_env: Env) -> u32 { 1 }
        pub fn is_paused(_env: Env) -> bool { false }
        pub fn get_latest_report(_env: Env, _symbol: Symbol, _feeder: Address) -> Option<(i128, u64)> {
            Some((i128::MAX, 0))
        }
    }
}
pub use extreme_high_oracle_impl::{ExtremeHighOracle, ExtremeHighOracleClient};

// ─── ExtremeLowOracle ────────────────────────────────────────────────────────

/// An oracle that returns `1` (the smallest positive price) for every
/// symbol. Models an oracle floor / dust attack.
///
/// **Vulnerability exercised (issue #115):** A settlement price of `1` on an
/// in-the-money call option where `strike_price > 1` means no payout; a put
/// option would be maximally in the money. Tests confirm payout bounds.
mod extreme_low_oracle_impl {
    use super::*;

    #[contract]
    pub struct ExtremeLowOracle;

    #[contractimpl]
    impl ExtremeLowOracle {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            env.storage().instance().set(&OracleKey::Admin, &admin);
        }

        /// Always returns `1`.
        pub fn get_price(_env: Env, _symbol: Symbol) -> Option<i128> {
            Some(1)
        }

        pub fn add_feeder(_env: Env, _feeder: Address) {}
        pub fn remove_feeder(_env: Env, _feeder: Address) {}
        pub fn report_price(_env: Env, _feeder: Address, _symbol: Symbol, _price: i128) {}
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool { false }
        pub fn get_feeder_count(_env: Env) -> u32 { 1 }
        pub fn get_max_staleness(_env: Env) -> u64 { 3600 }
        pub fn get_min_reports(_env: Env) -> u32 { 1 }
        pub fn is_paused(_env: Env) -> bool { false }
        pub fn get_latest_report(_env: Env, _symbol: Symbol, _feeder: Address) -> Option<(i128, u64)> {
            Some((1, 0))
        }
    }
}
pub use extreme_low_oracle_impl::{ExtremeLowOracle, ExtremeLowOracleClient};

// ─── RevertingOracle ─────────────────────────────────────────────────────────

/// An oracle whose `get_price` always panics. Models a bricked or
/// maliciously deployed oracle contract.
///
/// **Vulnerability exercised (issue #115):** `set_settlement_price_from_oracle`
/// must not leave the series in a partially-updated state if the oracle
/// call panics. The whole transaction should revert, not silently settle at
/// price 0. Tests confirm atomicity.
mod reverting_oracle_impl {
    use super::*;

    #[contract]
    pub struct RevertingOracle;

    #[contractimpl]
    impl RevertingOracle {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            env.storage().instance().set(&OracleKey::Admin, &admin);
        }

        /// Always panics.
        pub fn get_price(env: Env, _symbol: Symbol) -> Option<i128> {
            panic_with_error!(env, OracleError::OracleReverted);
        }

        pub fn add_feeder(_env: Env, _feeder: Address) {}
        pub fn remove_feeder(_env: Env, _feeder: Address) {}
        pub fn report_price(_env: Env, _feeder: Address, _symbol: Symbol, _price: i128) {}
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool { false }
        pub fn get_feeder_count(_env: Env) -> u32 { 0 }
        pub fn get_max_staleness(_env: Env) -> u64 { 3600 }
        pub fn get_min_reports(_env: Env) -> u32 { 1 }
        pub fn is_paused(_env: Env) -> bool { false }
        pub fn get_latest_report(_env: Env, _symbol: Symbol, _feeder: Address) -> Option<(i128, u64)> { None }
    }
}
pub use reverting_oracle_impl::{RevertingOracle, RevertingOracleClient};

// ─── StaleOracle ─────────────────────────────────────────────────────────────

/// An oracle that has a stored price but reports it with a timestamp of 0
/// (epoch), so it will always appear stale to any consumer that checks
/// freshness. `get_price` still returns `Some(price)` because this oracle
/// does not check staleness itself — only the consumer's staleness guard
/// should reject it.
///
/// **Vulnerability exercised (issue #115):** If options_market's cross-contract
/// oracle call doesn't verify freshness (it delegates to price_oracle, which
/// does its own staleness check), this confirms what happens when the oracle
/// aggregate is valid but ancient. The real price_oracle would return `None`;
/// this adversary returns a stale `Some` to probe whether the consumer
/// double-checks.
mod stale_oracle_impl {
    use super::*;

    #[contract]
    pub struct StaleOracle;

    /// Stored price in StaleOracle: just a single fixed value.
    const STALE_PRICE: i128 = 10_000_000; // $1.00 at PRICE_PRECISION 1e7

    #[contractimpl]
    impl StaleOracle {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            env.storage().instance().set(&OracleKey::Admin, &admin);
        }

        /// Returns a price with a timestamp of 0 (always stale).
        pub fn get_price(_env: Env, _symbol: Symbol) -> Option<i128> {
            Some(STALE_PRICE)
        }

        pub fn get_latest_report(_env: Env, _symbol: Symbol, _feeder: Address) -> Option<(i128, u64)> {
            // timestamp = 0: stale since epoch
            Some((STALE_PRICE, 0))
        }

        pub fn add_feeder(_env: Env, _feeder: Address) {}
        pub fn remove_feeder(_env: Env, _feeder: Address) {}
        pub fn report_price(_env: Env, _feeder: Address, _symbol: Symbol, _price: i128) {}
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool { false }
        pub fn get_feeder_count(_env: Env) -> u32 { 1 }
        pub fn get_max_staleness(_env: Env) -> u64 { 3600 }
        pub fn get_min_reports(_env: Env) -> u32 { 1 }
        pub fn is_paused(_env: Env) -> bool { false }
    }
}
pub use stale_oracle_impl::{StaleOracle, StaleOracleClient};

// ─── NegativePriceOracle ─────────────────────────────────────────────────────

/// An oracle that returns a negative price (`-1`). The Zenith `price_oracle`
/// rejects non-positive prices at `report_price` time, but this adversary
/// bypasses that guard by implementing the interface directly.
///
/// **Vulnerability exercised (issue #115):** `set_settlement_price_from_oracle`
/// receives the value from the oracle without re-validating sign. If payout
/// math is `settlement_price - strike_price` and settlement_price is -1,
/// the difference underflows. Confirms options_market validates the oracle's
/// return value before using it in arithmetic.
mod negative_price_oracle_impl {
    use super::*;

    #[contract]
    pub struct NegativePriceOracle;

    #[contractimpl]
    impl NegativePriceOracle {
        pub fn initialize(env: Env, admin: Address) {
            admin.require_auth();
            env.storage().instance().set(&OracleKey::Admin, &admin);
        }

        /// Always returns `-1` — an invalid price that bypasses oracle-side
        /// validation and should be caught on the consumer side.
        pub fn get_price(_env: Env, _symbol: Symbol) -> Option<i128> {
            Some(-1)
        }

        pub fn add_feeder(_env: Env, _feeder: Address) {}
        pub fn remove_feeder(_env: Env, _feeder: Address) {}
        pub fn report_price(_env: Env, _feeder: Address, _symbol: Symbol, _price: i128) {}
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool { false }
        pub fn get_feeder_count(_env: Env) -> u32 { 1 }
        pub fn get_max_staleness(_env: Env) -> u64 { 3600 }
        pub fn get_min_reports(_env: Env) -> u32 { 1 }
        pub fn is_paused(_env: Env) -> bool { false }
        pub fn get_latest_report(_env: Env, _symbol: Symbol, _feeder: Address) -> Option<(i128, u64)> {
            Some((-1, 0))
        }
    }
}
pub use negative_price_oracle_impl::{NegativePriceOracle, NegativePriceOracleClient};
