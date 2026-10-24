//! Settlement Keeper CLI
//!
//! Watches for expired option series and settles them permissionlessly via
//! `options_market::set_settlement_price_from_oracle`. Refuses to settle when
//! the `price_oracle` aggregate is stale, below quorum, or the symbol is
//! unregistered.
//!
//! Usage:
//!   settlement-keeper --once
//!   settlement-keeper --dry-run --once
//!   settlement-keeper --poll-interval 30
//!
//! Identity is read from `SETTLEMENT_KEEPER_SECRET` (env) or `--secret` flag.
//! Key material is never written to disk or logged.

use std::collections::HashSet;
use std::env;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug, Clone)]
#[command(
    name = "settlement-keeper",
    about = "Permissionless expiry settlement keeper for options_market"
)]
struct Cli {
    /// RPC endpoint for the Soroban network.
    #[arg(long, env = "SOROBAN_RPC_URL", default_value = "http://localhost:8000/soroban/rpc")]
    rpc_url: String,

    /// Network passphrase.
    #[arg(
        long,
        env = "SOROBAN_NETWORK_PASSPHRASE",
        default_value = "Standalone Network ; February 2017"
    )]
    network_passphrase: String,

    /// Contract id of `options_market`.
    #[arg(long, env = "OPTIONS_MARKET_CONTRACT_ID")]
    options_market: String,

    /// Contract id of `price_oracle`.
    #[arg(long, env = "PRICE_ORACLE_CONTRACT_ID")]
    price_oracle: String,

    /// Secret key (S...) for the keeper account. Prefer the env var.
    #[arg(long, env = "SETTLEMENT_KEEPER_SECRET", hide_env_values = true)]
    secret: Option<String>,

    /// Print intended calls and observed oracle state without submitting.
    #[arg(long)]
    dry_run: bool,

    /// Run a single pass and exit.
    #[arg(long)]
    once: bool,

    /// Polling interval in seconds (ignored with --once).
    #[arg(long, default_value_t = 60)]
    poll_interval: u64,

    /// Maximum number of retry attempts for transient RPC failures.
    #[arg(long, default_value_t = 5)]
    max_retries: u32,

    /// Base backoff in milliseconds for retries.
    #[arg(long, default_value_t = 500)]
    backoff_ms: u64,
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A series as reported by `options_market`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SeriesInfo {
    series_id: u64,
    symbol: String,
    /// Ledger close time (unix seconds) at which the series expires.
    expiry: u64,
    /// Whether the series has already been settled.
    settled: bool,
}

/// Snapshot of the `price_oracle` aggregate for a symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OracleSnapshot {
    /// Whether the symbol is registered with the oracle.
    registered: bool,
    /// Age of the aggregate in seconds, measured against ledger close time.
    age_secs: u64,
    /// Number of reports backing the aggregate.
    report_count: u32,
    /// Minimum reports required for a valid aggregate.
    min_reports: u32,
    /// Maximum acceptable age in seconds.
    max_age_secs: u64,
    /// Aggregate price (informational; not used for the decision).
    price: i128,
}

/// Decision returned by the pure decision function.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Decision {
    Settle { price: i128 },
    Skip { reason: String },
}

/// Pure decision function: given the ledger close time, the series, and the
/// oracle snapshot, decide whether to settle.
///
/// This function has no I/O and is fully unit-testable.
fn decide(ledger_close_time: u64, series: &SeriesInfo, oracle: &OracleSnapshot) -> Decision {
    if series.settled {
        return Decision::Skip {
            reason: "series already settled".to_string(),
        };
    }
    if ledger_close_time < series.expiry {
        return Decision::Skip {
            reason: format!(
                "not yet expired (expiry {}, ledger time {})",
                series.expiry, ledger_close_time
            ),
        };
    }
    if !oracle.registered {
        return Decision::Skip {
            reason: format!("symbol '{}' is not registered with price_oracle", series.symbol),
        };
    }
    if oracle.report_count < oracle.min_reports {
        return Decision::Skip {
            reason: format!(
                "aggregate below quorum: {} reports < min_reports {}",
                oracle.report_count, oracle.min_reports
            ),
        };
    }
    if oracle.age_secs > oracle.max_age_secs {
        return Decision::Skip {
            reason: format!(
                "aggregate stale: age {}s > max {}s",
                oracle.age_secs, oracle.max_age_secs
            ),
        };
    }
    Decision::Settle { price: oracle.price }
}

// ---------------------------------------------------------------------------
// RPC abstraction
// ---------------------------------------------------------------------------

/// Minimal read-only RPC client abstraction. In a full implementation this
/// would wrap `soroban_client` / `stellar_rpc_client`. Kept as a trait so the
/// keeper loop can be exercised with a fake in tests.
trait Rpc {
    /// Fetch the current ledger close time (unix seconds).
    fn ledger_close_time(&self) -> Result<u64>;
    /// List series that are expired or approaching expiry.
    fn list_series(&self) -> Result<Vec<SeriesInfo>>;
    /// Fetch the oracle aggregate snapshot for a symbol.
    fn oracle_snapshot(&self, symbol: &str) -> Result<OracleSnapshot>;
    /// Submit `set_settlement_price_from_oracle` for a series.
    fn submit_settlement(&self, series_id: u64, price: i128) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Keeper loop
// ---------------------------------------------------------------------------

struct Keeper<R: Rpc> {
    rpc: R,
    dry_run: bool,
    max_retries: u32,
    backoff: Duration,
    /// Series ids we have already submitted (or attempted) in this process.
    submitted: HashSet<u64>,
}

impl<R: Rpc> Keeper<R> {
    fn new(rpc: R, dry_run: bool, max_retries: u32, backoff: Duration) -> Self {
        Self {
            rpc,
            dry_run,
            max_retries,
            backoff,
            submitted: HashSet::new(),
        }
    }

    /// Run a single pass over all series. Returns the number of settlements
    /// submitted (or would-be submitted in dry-run).
    fn run_once(&mut self) -> Result<usize> {
        // Trust ledger close time, not local wall clock.
        let ledger_close_time = self
            .rpc
            .ledger_close_time()
            .context("failed to fetch ledger close time")?;

        let series_list = self
            .rpc
            .list_series()
            .context("failed to list series")?;

        let mut settled_count = 0usize;

        for series in &series_list {
            if self.submitted.contains(&series.series_id) {
                // Never submit twice for the same series in one process.
                continue;
            }

            let oracle = match self.rpc.oracle_snapshot(&series.symbol) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "[keeper] series {}: failed to fetch oracle snapshot: {:#}",
                        series.series_id, e
                    );
                    continue;
                }
            };

            match decide(ledger_close_time, series, &oracle) {
                Decision::Skip { reason } => {
                    println!(
                        "[keeper] series {} ({}): skip — {}",
                        series.series_id, series.symbol, reason
                    );
                }
                Decision::Settle { price } => {
                    println!(
                        "[keeper] series {} ({}): settle at price {} (age {}s, {} reports)",
                        series.series_id, series.symbol, price, oracle.age_secs, oracle.report_count
                    );
                    if self.dry_run {
                        println!(
                            "[keeper] dry-run: would call set_settlement_price_from_oracle(series={}, price={})",
                            series.series_id, price
                        );
                        self.submitted.insert(series.series_id);
                        settled_count += 1;
                        continue;
                    }
                    match self.submit_with_retry(series.series_id, price) {
                        Ok(()) => {
                            self.submitted.insert(series.series_id);
                            settled_count += 1;
                        }
                        Err(e) => {
                            eprintln!(
                                "[keeper] series {}: settlement failed: {:#}",
                                series.series_id, e
                            );
                        }
                    }
                }
            }
        }

        Ok(settled_count)
    }

    fn submit_with_retry(&self, series_id: u64, price: i128) -> Result<()> {
        let mut attempt = 0u32;
        let mut delay = self.backoff;
        loop {
            attempt += 1;
            match self.rpc.submit_settlement(series_id, price) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if attempt >= self.max_retries {
                        return Err(e.context(format!(
                            "giving up after {} attempts",
                            attempt
                        )));
                    }
                    eprintln!(
                        "[keeper] series {}: attempt {}/{} failed: {:#}; retrying in {:?}",
                        series_id, attempt, self.max_retries, e, delay
                    );
                    std::thread::sleep(delay);
                    delay = delay.saturating_mul(2);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.secret.is_none() && !cli.dry_run {
        bail!(
            "no signing identity provided: set SETTLEMENT_KEEPER_SECRET or pass --secret \
             (or use --dry-run)"
        );
    }

    // NOTE: In a full implementation this constructs a real RPC client from
    // `cli.rpc_url`, `cli.network_passphrase`, and the contract ids. The
    // concrete client is intentionally omitted here; the keeper logic above
    // is transport-agnostic.
    let rpc = RealRpc::new(
        cli.rpc_url.clone(),
        cli.network_passphrase.clone(),
        cli.options_market.clone(),
        cli.price_oracle.clone(),
        cli.secret.clone(),
    )?;

    let mut keeper = Keeper::new(
        rpc,
        cli.dry_run,
        cli.max_retries,
        Duration::from_millis(cli.backoff_ms),
    );

    if cli.once {
        let n = keeper.run_once()?;
        println!("[keeper] pass complete: {} settlement(s)", n);
        return Ok(());
    }

    let interval = Duration::from_secs(cli.poll_interval.max(1));
    loop {
        match keeper.run_once() {
            Ok(n) => println!("[keeper] pass complete: {} settlement(s)", n),
            Err(e) => eprintln!("[keeper] pass failed: {:#}", e),
        }
        std::thread::sleep(interval);
    }
}

// ---------------------------------------------------------------------------
// Real RPC client (stub)
// ---------------------------------------------------------------------------

/// Placeholder for the concrete Soroban RPC client. The real implementation
/// wires up `soroban_client` and invokes the contract entrypoints. Kept as a
/// distinct type so the keeper logic remains testable.
struct RealRpc {
    #[allow(dead_code)]
    rpc_url: String,
    #[allow(dead_code)]
    network_passphrase: String,
    #[allow(dead_code)]
    options_market: String,
    #[allow(dead_code)]
    price_oracle: String,
    #[allow(dead_code)]
    secret: Option<String>,
}

impl RealRpc {
    fn new(
        rpc_url: String,
        network_passphrase: String,
        options_market: String,
        price_oracle: String,
        secret: Option<String>,
    ) -> Result<Self> {
        if options_market.is_empty() {
            bail!("options_market contract id is required");
        }
        if price_oracle.is_empty() {
            bail!("price_oracle contract id is required");
        }
        Ok(Self {
            rpc_url,
            network_passphrase,
            options_market,
            price_oracle,
            secret,
        })
    }
}

impl Rpc for RealRpc {
    fn ledger_close_time(&self) -> Result<u64> {
        Err(anyhow!("RealRpc::ledger_close_time not wired up in this build"))
    }
    fn list_series(&self) -> Result<Vec<SeriesInfo>> {
        Err(anyhow!("RealRpc::list_series not wired up in this build"))
    }
    fn oracle_snapshot(&self, _symbol: &str) -> Result<OracleSnapshot> {
        Err(anyhow!("RealRpc::oracle_snapshot not wired up in this build"))
    }
    fn submit_settlement(&self, _series_id: u64, _price: i128) -> Result<()> {
        Err(anyhow!("RealRpc::submit_settlement not wired up in this build"))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn series(settled: bool) -> SeriesInfo {
        SeriesInfo {
            series_id: 1,
            symbol: "BTC/USD".to_string(),
            expiry: 1_000,
            settled,
        }
    }

    fn oracle() -> OracleSnapshot {
        OracleSnapshot {
            registered: true,
            age_secs: 10,
            report_count: 5,
            min_reports: 3,
            max_age_secs: 60,
            price: 42_000,
        }
    }

    #[test]
    fn settles_when_fresh_and_quorum() {
        let d = decide(1_000, &series(false), &oracle());
        assert_eq!(d, Decision::Settle { price: 42_000 });
    }

    #[test]
    fn skips_when_not_expired() {
        let d = decide(999, &series(false), &oracle());
        match d {
            Decision::Skip { reason } => assert!(reason.contains("not yet expired")),
            _ => panic!("expected skip"),
        }
    }

    #[test]
    fn skips_when_already_settled() {
        let d = decide(2_000, &series(true), &oracle());
        match d {
            Decision::Skip { reason } => assert!(reason.contains("already settled")),
            _ => panic!("expected skip"),
        }
    }

    #[test]
    fn skips_when_stale() {
        let mut o = oracle();
        o.age_secs = 61;
        let d = decide(2_000, &series(false), &o);
        match d {
            Decision::Skip { reason } => assert!(reason.contains("stale")),
            _ => panic!("expected skip"),
        }
    }

    #[test]
    fn skips_when_below_quorum() {
        let mut o = oracle();
        o.report_count = 2;
        let d = decide(2_000, &series(false), &o);
        match d {
            Decision::Skip { reason } => assert!(reason.contains("below quorum")),
            _ => panic!("expected skip"),
        }
    }

    #[test]
    fn skips_when_unregistered() {
        let mut o = oracle();
        o.registered = false;
        let d = decide(2_000, &series(false), &o);
        match d {
            Decision::Skip { reason } => assert!(reason.contains("not registered")),
            _ => panic!("expected skip"),
        }
    }

    #[test]
    fn skips_at_exact_staleness_boundary() {
        // age == max is acceptable; age > max is stale.
        let mut o = oracle();
        o.age_secs = o.max_age_secs;
        assert!(matches!(decide(2_000, &series(false), &o), Decision::Settle { .. }));
        o.age_secs = o.max_age_secs + 1;
        assert!(matches!(decide(2_000, &series(false), &o), Decision::Skip { .. }));
    }

    #[test]
    fn skips_at_exact_quorum_boundary() {
        let mut o = oracle();
        o.report_count = o.min_reports;
        assert!(matches!(decide(2_000, &series(false), &o), Decision::Settle { .. }));
        o.report_count = o.min_reports - 1;
        assert!(matches!(decide(2_000, &series(false), &o), Decision::Skip { .. }));
    }

    // --- Keeper loop tests with a fake RPC ---

    struct FakeRpc {
        ledger_time: u64,
        series: Vec<SeriesInfo>,
        oracle: OracleSnapshot,
        submissions: std::cell::RefCell<Vec<(u64, i128)>>,
    }

    impl Rpc for FakeRpc {
        fn ledger_close_time(&self) -> Result<u64> {
            Ok(self.ledger_time)
        }
        fn list_series(&self) -> Result<Vec<SeriesInfo>> {
            Ok(self.series.clone())
        }
        fn oracle_snapshot(&self, _symbol: &str) -> Result<OracleSnapshot> {
            Ok(self.oracle.clone())
        }
        fn submit_settlement(&self, series_id: u64, price: i128) -> Result<()> {
            self.submissions.borrow_mut().push((series_id, price));
            Ok(())
        }
    }

    #[test]
    fn keeper_submits_once_per_series() {
        let rpc = FakeRpc {
            ledger_time: 2_000,
            series: vec![series(false)],
            oracle: oracle(),
            submissions: std::cell::RefCell::new(Vec::new()),
        };
        let mut keeper = Keeper::new(rpc, false, 3, Duration::from_millis(1));
        keeper.run_once().unwrap();
        keeper.run_once().unwrap();
        assert_eq!(keeper.rpc.submissions.borrow().len(), 1);
    }

    #[test]
    fn keeper_dry_run_does_not_submit() {
        let rpc = FakeRpc {
            ledger_time: 2_000,
            series: vec![series(false)],
            oracle: oracle(),
            submissions: std::cell::RefCell::new(Vec::new()),
        };
        let mut keeper = Keeper::new(rpc, true, 3, Duration::from_millis(1));
        let n = keeper.run_once().unwrap();
        assert_eq!(n, 1);
        assert!(keeper.rpc.submissions.borrow().is_empty());
    }
}