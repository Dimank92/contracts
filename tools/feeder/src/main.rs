use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zenith_feeder::config::Config;
use zenith_feeder::monitor::{Monitor, Status};
use zenith_feeder::submit::StellarCliSubmitter;
use zenith_feeder::{log, run_round};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let once = args.iter().any(|a| a == "--once");
    let path = match args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
    {
        Some(p) => p,
        None => {
            eprintln!("usage: zenith-feeder --config <file.json> [--dry-run] [--once]");
            std::process::exit(2);
        }
    };
    let cfg = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|s| Config::parse(&s))
        .unwrap_or_else(|e| {
            log("error", "invalid config", serde_json::json!({ "error": e }));
            std::process::exit(2);
        });
    let secret = if dry_run {
        String::new()
    } else {
        cfg.load_secret().unwrap_or_else(|e| {
            log(
                "error",
                "cannot load feeder key",
                serde_json::json!({ "error": e }),
            );
            std::process::exit(2);
        })
    };
    let submitter = StellarCliSubmitter::new(
        &cfg.rpc_url,
        &cfg.network_passphrase,
        &cfg.oracle_contract_id,
        secret,
    );
    let mut monitor = Monitor::new(now());
    loop {
        let t = now();
        // Same interval => same id => a repeated round is idempotent.
        let interval_id = t / cfg.submit_interval_secs;
        match run_round(
            &cfg,
            &submitter,
            &mut monitor,
            interval_id,
            t,
            dry_run,
            &std::thread::sleep,
        ) {
            Status::Ok => {}
            Status::Warn => log(
                "warn",
                "last accepted report nearing staleness window",
                serde_json::json!({}),
            ),
            Status::Exit(why) if !dry_run => {
                log("error", "exiting", serde_json::json!({ "reason": why }));
                std::process::exit(1);
            }
            Status::Exit(_) => {}
        }
        if once || dry_run {
            return;
        }
        std::thread::sleep(Duration::from_secs(cfg.submit_interval_secs));
    }
}
