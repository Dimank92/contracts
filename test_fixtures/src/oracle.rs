//! Oracle/feeder test fixture: an in-process harness that holds feeder
//! reports in Soroban-native storage without deploying a contract.
//!
//! Useful in crates that can't or don't want to take a full
//! `price_oracle` dev-dependency. Respects `max_staleness` and
//! `min_reports` the same way the real oracle does.
//!
//! # Example
//! ```rust,ignore
//! let env = Env::default();
//! let mut oracle = OracleHarness::new(&env);
//! let feeder = oracle.add_feeder(&env);
//! oracle.report(&env, &feeder, "XLM", 1_500_000);
//! assert_eq!(oracle.get_price(&env, "XLM"), Some(1_500_000));
//!
//! // Advance time past max_staleness to simulate a stale report:
//! env.ledger().with_mut(|l| l.timestamp += 7_200);
//! assert!(oracle.get_price(&env, "XLM").is_none());
//! ```

use soroban_sdk::{contracttype, Address, Env, Symbol, Vec};

/// One feeder's price observation, stored in a Soroban-compatible struct.
#[contracttype(export = false)]
#[derive(Clone)]
pub struct PriceReport {
    pub feeder: Address,
    pub symbol: Symbol,
    pub price: i128,
    pub timestamp: u64,
}

/// An in-process oracle harness: tracks feeders and reports in Soroban
/// `Vec`s so the harness works inside the Soroban test environment without
/// spinning up a contract.
pub struct OracleHarness {
    /// Authorized feeders.
    pub feeders: Vec<Address>,
    /// All submitted reports (newest last).
    pub reports: Vec<PriceReport>,
    /// Maximum age (seconds) of a report before it's considered stale.
    pub max_staleness: u64,
    /// Minimum number of fresh reports required before `get_price` returns
    /// a value (mirrors `min_reports` on the real price_oracle).
    pub min_reports: u32,
}

impl OracleHarness {
    /// Create a new harness with default settings (1 h staleness, quorum 1).
    pub fn new(env: &Env) -> Self {
        OracleHarness {
            feeders: Vec::new(env),
            reports: Vec::new(env),
            max_staleness: 3_600,
            min_reports: 1,
        }
    }

    /// Register a new feeder (generated address) and return its address.
    pub fn add_feeder(&mut self, env: &Env) -> Address {
        let feeder = Address::generate(env);
        self.feeders.push_back(feeder.clone());
        feeder
    }

    /// Record a price observation from `feeder` for `symbol` at the current
    /// ledger timestamp. Non-positive prices are not validated here — tests
    /// that want to exercise that path should use the real contract.
    pub fn report(&mut self, env: &Env, feeder: &Address, symbol: &str, price: i128) {
        self.reports.push_back(PriceReport {
            feeder: feeder.clone(),
            symbol: Symbol::new(env, symbol),
            price,
            timestamp: env.ledger().timestamp(),
        });
    }

    /// Compute the median of fresh, authorized reports for `symbol`.
    /// Returns `None` when fewer than `min_reports` fresh reports exist.
    pub fn get_price(&self, env: &Env, symbol: &str) -> Option<i128> {
        let sym = Symbol::new(env, symbol);
        let now = env.ledger().timestamp();

        // Collect fresh prices from authorized feeders.
        let mut fresh: Vec<i128> = Vec::new(env);
        for report in self.reports.iter() {
            if report.symbol != sym {
                continue;
            }
            // Only count authorized feeders.
            let is_authorized = self.feeders.iter().any(|f| f == report.feeder);
            if !is_authorized {
                continue;
            }
            // Skip stale reports.
            if now.saturating_sub(report.timestamp) > self.max_staleness {
                continue;
            }
            fresh.push_back(report.price);
        }

        let n = fresh.len();
        if n < self.min_reports {
            return None;
        }

        // Insertion sort (matches the real oracle's approach for small N).
        let mut sorted = fresh;
        for i in 1..sorted.len() {
            let mut j = i;
            while j > 0 {
                let prev = sorted.get(j - 1).unwrap();
                let cur = sorted.get(j).unwrap();
                if prev <= cur {
                    break;
                }
                sorted.set(j - 1, cur);
                sorted.set(j, prev);
                j -= 1;
            }
        }

        let mid = n / 2;
        if n % 2 == 1 {
            sorted.get(mid)
        } else {
            let a = sorted.get(mid - 1)?;
            let b = sorted.get(mid)?;
            Some((a + b) / 2)
        }
    }
}
