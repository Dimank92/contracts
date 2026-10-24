//! Reference feeder daemon for `price_oracle`.
//!
//! Operators supply prices (this crate never sources them from venues);
//! the daemon scales them to each symbol's decimals, submits one report per
//! symbol per interval, retries with bounded exponential backoff, and
//! tracks its own staleness. Modules: [`config`], [`report`], [`submit`],
//! [`monitor`].

pub mod config;
pub mod monitor;
pub mod report;
pub mod submit;

use std::time::Duration;

use config::Config;
use monitor::{Monitor, Status};
use report::Report;
use submit::{submit_with_retry, SubmitError, Submitter};

/// Emits one JSON line. Never pass secrets here.
pub fn log(level: &str, msg: &str, fields: serde_json::Value) {
    let line = serde_json::json!({ "level": level, "msg": msg, "fields": fields });
    println!("{line}");
}

/// One submission round. Idempotent per interval: `interval_id` is
/// embedded in each report so a repeated round carries identical payloads.
/// Returns the monitor status; `Exit` means the supervisor should restart us.
pub fn run_round(
    cfg: &Config,
    submitter: &dyn Submitter,
    monitor: &mut Monitor,
    interval_id: u64,
    now: u64,
    dry_run: bool,
    sleep: &dyn Fn(Duration),
) -> Status {
    for sym in &cfg.symbols {
        let report = match Report::build(sym, interval_id) {
            Ok(r) => r,
            Err(e) => {
                // Degrade gracefully: skip only this symbol.
                log(
                    "warn",
                    "symbol skipped",
                    serde_json::json!({"symbol": sym.name, "error": e}),
                );
                continue;
            }
        };
        if dry_run {
            log("info", "dry-run report", report.to_json());
            continue;
        }
        match submit_with_retry(
            submitter,
            &report,
            cfg.max_retries,
            cfg.backoff_base_ms,
            sleep,
        ) {
            Ok(()) => {
                monitor.record_accepted(&sym.name, now);
                log("info", "report accepted", report.to_json());
            }
            Err(SubmitError::Rejected(why)) => {
                monitor.record_rejected(&sym.name);
                log(
                    "error",
                    "report rejected by contract",
                    serde_json::json!({"symbol": sym.name, "reason": why}),
                );
            }
            Err(e) => log(
                "error",
                "submission failed",
                serde_json::json!({"symbol": sym.name, "error": e.to_string()}),
            ),
        }
    }
    monitor.check(now, cfg.staleness_window_secs, cfg.staleness_warn_secs)
}
