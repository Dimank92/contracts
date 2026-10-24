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
//!
//! ## Quorum adversaries (price_oracle aggregation)
//!
//! The structs above are consumer-side adversaries: they implement the
//! oracle interface directly and are used to probe `options_market`. The
//! module below instead attacks the *real* `price_oracle` aggregation logic
//! through its public entrypoints (`add_feeder`, `report_price`,
//! `get_price`). See [`quorum`] for the deterministic suite.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, Address, Env, Symbol,
};

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

pub use always_none_oracle::{AlwaysNoneOracle, AlwaysNoneOracleClient};

mod always_none_oracle {
    use super::*;

    /// An oracle that returns `None` for every `get_price` call, regardless
    /// of how many feeders have reported. Simulates a situation where quorum
    /// has never been reached or all reports have gone stale.
    ///
    /// **Vulnerability exercised (issue #115):** `set_settlement_price_from_oracle`
    /// must handle `None` from the oracle gracefully — i.e. not interpret a
    /// missing price as zero and settle at the wrong value. Tests confirm it
    /// propagates an error / reverts instead.
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
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool {
            false
        }
        pub fn get_feeder_count(_env: Env) -> u32 {
            0
        }
        pub fn get_max_staleness(_env: Env) -> u64 {
            3600
        }
        pub fn get_min_reports(_env: Env) -> u32 {
            1
        }
        pub fn is_paused(_env: Env) -> bool {
            false
        }
        pub fn get_latest_report(
            _env: Env,
            _symbol: Symbol,
            _feeder: Address,
        ) -> Option<(i128, u64)> {
            None
        }
    }
}

// ─── ExtremeHighOracle ───────────────────────────────────────────────────────

pub use extreme_high_oracle::{ExtremeHighOracle, ExtremeHighOracleClient};

mod extreme_high_oracle {
    use super::*;

    /// An oracle that returns `i128::MAX` as the settlement price for every
    /// symbol. Models a compromised or buggy feeder reporting an impossibly
    /// high price.
    ///
    /// **Vulnerability exercised (issue #115):** Settlement payout arithmetic
    /// (strike_price × contracts / PRICE_PRECISION) performed on `i128::MAX`
    /// will overflow unless checked. Confirms overflow-safe arithmetic in
    /// options_market's exercise path.
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
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool {
            false
        }
        pub fn get_feeder_count(_env: Env) -> u32 {
            1
        }
        pub fn get_max_staleness(_env: Env) -> u64 {
            3600
        }
        pub fn get_min_reports(_env: Env) -> u32 {
            1
        }
        pub fn is_paused(_env: Env) -> bool {
            false
        }
        pub fn get_latest_report(
            _env: Env,
            _symbol: Symbol,
            _feeder: Address,
        ) -> Option<(i128, u64)> {
            Some((i128::MAX, 0))
        }
    }
}

// ─── ExtremeLowOracle ────────────────────────────────────────────────────────

pub use extreme_low_oracle::{ExtremeLowOracle, ExtremeLowOracleClient};

mod extreme_low_oracle {
    use super::*;

    /// An oracle that returns `1` (the smallest positive price) for every
    /// symbol. Models an oracle floor / dust attack.
    ///
    /// **Vulnerability exercised (issue #115):** A settlement price of `1` on an
    /// in-the-money call option where `strike_price > 1` means no payout; a put
    /// option would be maximally in the money. Tests confirm payout bounds.
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
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool {
            false
        }
        pub fn get_feeder_count(_env: Env) -> u32 {
            1
        }
        pub fn get_max_staleness(_env: Env) -> u64 {
            3600
        }
        pub fn get_min_reports(_env: Env) -> u32 {
            1
        }
        pub fn is_paused(_env: Env) -> bool {
            false
        }
        pub fn get_latest_report(
            _env: Env,
            _symbol: Symbol,
            _feeder: Address,
        ) -> Option<(i128, u64)> {
            Some((1, 0))
        }
    }
}

// ─── RevertingOracle ─────────────────────────────────────────────────────────

pub use reverting_oracle::{RevertingOracle, RevertingOracleClient};

mod reverting_oracle {
    use super::*;

    /// An oracle whose `get_price` always panics. Models a bricked or
    /// maliciously deployed oracle contract.
    ///
    /// **Vulnerability exercised (issue #115):** `set_settlement_price_from_oracle`
    /// must not leave the series in a partially-updated state if the oracle
    /// call panics. The whole transaction should revert, not silently settle at
    /// price 0. Tests confirm atomicity.
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
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool {
            false
        }
        pub fn get_feeder_count(_env: Env) -> u32 {
            0
        }
        pub fn get_max_staleness(_env: Env) -> u64 {
            3600
        }
        pub fn get_min_reports(_env: Env) -> u32 {
            1
        }
        pub fn is_paused(_env: Env) -> bool {
            false
        }
        pub fn get_latest_report(
            _env: Env,
            _symbol: Symbol,
            _feeder: Address,
        ) -> Option<(i128, u64)> {
            None
        }
    }
}

// ─── StaleOracle ─────────────────────────────────────────────────────────────

pub use stale_oracle::{StaleOracle, StaleOracleClient};

mod stale_oracle {
    use super::*;

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

        pub fn get_latest_report(
            _env: Env,
            _symbol: Symbol,
            _feeder: Address,
        ) -> Option<(i128, u64)> {
            // timestamp = 0: stale since epoch
            Some((STALE_PRICE, 0))
        }

        pub fn add_feeder(_env: Env, _feeder: Address) {}
        pub fn remove_feeder(_env: Env, _feeder: Address) {}
        pub fn report_price(_env: Env, _feeder: Address, _symbol: Symbol, _price: i128) {}
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool {
            false
        }
        pub fn get_feeder_count(_env: Env) -> u32 {
            1
        }
        pub fn get_max_staleness(_env: Env) -> u64 {
            3600
        }
        pub fn get_min_reports(_env: Env) -> u32 {
            1
        }
        pub fn is_paused(_env: Env) -> bool {
            false
        }
    }
}

// ─── NegativePriceOracle ─────────────────────────────────────────────────────

pub use negative_price_oracle::{NegativePriceOracle, NegativePriceOracleClient};

mod negative_price_oracle {
    use super::*;

    /// An oracle that returns a negative price (`-1`). The Zenith `price_oracle`
    /// rejects non-positive prices at `report_price` time, but this adversary
    /// bypasses that guard by implementing the interface directly.
    ///
    /// **Vulnerability exercised (issue #115):** `set_settlement_price_from_oracle`
    /// receives the value from the oracle without re-validating sign. If payout
    /// math is `settlement_price - strike_price` and settlement_price is -1,
    /// the difference underflows. Confirms options_market validates the oracle's
    /// return value before using it in arithmetic.
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
        pub fn is_feeder(_env: Env, _feeder: Address) -> bool {
            false
        }
        pub fn get_feeder_count(_env: Env) -> u32 {
            1
        }
        pub fn get_max_staleness(_env: Env) -> u64 {
            3600
        }
        pub fn get_min_reports(_env: Env) -> u32 {
            1
        }
        pub fn is_paused(_env: Env) -> bool {
            false
        }
        pub fn get_latest_report(
            _env: Env,
            _symbol: Symbol,
            _feeder: Address,
        ) -> Option<(i128, u64)> {
            Some((-1, 0))
        }
    }
}

// ─── quorum: stale / minority feeder sets vs min_reports ─────────────────────

/// Adversarial suite for `price_oracle` aggregation.
///
/// Every scenario is built exclusively through the public `price_oracle`
/// entrypoints (`initialize`, `add_feeder`, `report_price`, `get_price`,
/// `get_min_reports`, `get_max_staleness`). No storage is poked directly.
///
/// Determinism: the ledger timestamp is driven explicitly via
/// `env.ledger().set_timestamp(..)`; there is no wall-clock or RNG use.
///
/// ## Scenarios
///
/// 1. **Single stale feeder cannot satisfy the quorum.** With
///    `min_reports = 2`, one fresh report plus one report that has aged past
///    `max_staleness` must *not* produce a price — the stale report is
///    excluded from the quorum count.
/// 2. **Two fresh feeders do satisfy the quorum.** The same feeder set, both
///    fresh, yields `Some(price)`.
/// 3. **A minority of fresh feeders cannot carry a stale majority.** With
///    `min_reports = 2` and three feeders where two are stale, the single
///    fresh report is insufficient.
/// 4. **A single feeder cannot satisfy a `min_reports = 2` quorum alone.**
///    Guards against the quorum being satisfied by report *count* rather
///    than distinct fresh feeders.
///
/// The suite is gated behind the `price_oracle` feature so that it is
/// compiled and executed in CI (see `test_adversaries/README.md` and the CI
/// report) rather than silently skipped. When the `price_oracle` crate is
/// not yet reconstructed the module is still listed in the CI report as
/// `gated` rather than omitted.
#[cfg(feature = "price_oracle")]
pub mod quorum {
    extern crate std;

    use price_oracle::{PriceOracleContract, PriceOracleContractClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        Address, Env, Symbol,
    };

    /// Base ledger timestamp used by every scenario (deterministic).
    const T0: u64 = 1_000_000;
    /// `max_staleness` configured on the oracle under test.
    const MAX_STALENESS: u64 = 3_600;
    /// Quorum size configured on the oracle under test.
    const MIN_REPORTS: u32 = 2;
    /// A fresh, in-band price.
    const FRESH_PRICE: i128 = 10_000_000; // $1.00 at PRICE_PRECISION 1e7
    /// A stale feeder's (different) price, used to detect it leaking in.
    const STALE_PRICE: i128 = 99_000_000;

    struct Harness {
        env: Env,
        client: PriceOracleContractClient<'static>,
        admin: Address,
        symbol: Symbol,
    }

    /// Deploy a real `price_oracle`, configure `min_reports` and
    /// `max_staleness`, and return a harness positioned at `T0`.
    fn setup() -> Harness {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(T0);

        let admin = Address::generate(&env);
        let contract_id = env.register(PriceOracleContract, ());
        let client = PriceOracleContractClient::new(&env, &contract_id);

        client.initialize(&admin, &MIN_REPORTS, &MAX_STALENESS);

        // Sanity: the configuration we rely on is what the contract reports.
        assert_eq!(client.get_min_reports(), MIN_REPORTS);
        assert_eq!(client.get_max_staleness(), MAX_STALENESS);

        let symbol = Symbol::new(&env, "AAPL");
        Harness { env, client, admin, symbol }
    }

    fn feeder(h: &Harness) -> Address {
        let f = Address::generate(&h.env);
        h.client.add_feeder(&h.admin, &f);
        assert!(h.client.is_feeder(&f));
        f
    }

    /// Scenario 1: one fresh + one stale report must not satisfy the quorum.
    #[test]
    fn single_stale_feeder_cannot_satisfy_quorum() {
        let h = setup();
        let fresh = feeder(&h);
        let stale = feeder(&h);

        // Fresh feeder reports at T0.
        h.client.report_price(&fresh, &h.symbol, &FRESH_PRICE);

        // Stale feeder reports at T0, then time advances past max_staleness.
        h.client.report_price(&stale, &h.symbol, &STALE_PRICE);
        h.env.ledger().set_timestamp(T0 + MAX_STALENESS + 1);

        // The fresh report is now also stale relative to the new timestamp,
        // so re-report from the fresh feeder to keep exactly one fresh and
        // one stale report in the set.
        h.client.report_price(&fresh, &h.symbol, &FRESH_PRICE);

        // Only one fresh report exists; the quorum of 2 is not met.
        assert_eq!(
            h.client.get_price(&h.symbol),
            None,
            "a single fresh feeder plus a stale feeder must not satisfy min_reports"
        );
    }

    /// Scenario 2: two fresh feeders do satisfy the quorum.
    #[test]
    fn two_fresh_feeders_satisfy_quorum() {
        let h = setup();
        let a = feeder(&h);
        let b = feeder(&h);

        h.client.report_price(&a, &h.symbol, &FRESH_PRICE);
        h.client.report_price(&b, &h.symbol, &FRESH_PRICE);

        assert_eq!(
            h.client.get_price(&h.symbol),
            Some(FRESH_PRICE),
            "two fresh feeders must satisfy a min_reports = 2 quorum"
        );
    }

    /// Scenario 3: a minority of fresh feeders cannot carry a stale majority.
    #[test]
    fn fresh_minority_cannot_carry_stale_majority() {
        let h = setup();
        let fresh = feeder(&h);
        let stale_a = feeder(&h);
        let stale_b = feeder(&h);

        // Two feeders report, then go stale.
        h.client.report_price(&stale_a, &h.symbol, &STALE_PRICE);
        h.client.report_price(&stale_b, &h.symbol, &STALE_PRICE);
        h.env.ledger().set_timestamp(T0 + MAX_STALENESS + 1);

        // A single fresh feeder reports after the staleness window.
        h.client.report_price(&fresh, &h.symbol, &FRESH_PRICE);

        // One fresh report < min_reports; the stale majority must not count.
        assert_eq!(
            h.client.get_price(&h.symbol),
            None,
            "a fresh minority must not satisfy the quorum via stale reports"
        );
    }

    /// Scenario 4: a single feeder cannot satisfy a `min_reports = 2` quorum
    /// on its own, even with repeated reports.
    #[test]
    fn single_feeder_cannot_satisfy_quorum_alone() {
        let h = setup();
        let only = feeder(&h);

        h.client.report_price(&only, &h.symbol, &FRESH_PRICE);
        // Re-reporting from the same feeder must not inflate the count.
        h.client.report_price(&only, &h.symbol, &FRESH_PRICE);

        assert_eq!(
            h.client.get_price(&h.symbol),
            None,
            "one feeder reporting twice must not satisfy a min_reports = 2 quorum"
        );
    }
}

/// Placeholder so the module is always present in the crate and appears in
/// the CI report even when the `price_oracle` feature is disabled. It is
/// intentionally *not* `#[ignore]`d: when the feature is off this compiles to
/// nothing, and the CI report lists `price_oracle::quorum` as `gated`.
#[cfg(not(feature = "price_oracle"))]
pub mod quorum {
    //! Gated: enable the `price_oracle` feature to run the aggregation
    //! adversary suite. See the crate README for the CI report entry.
}
