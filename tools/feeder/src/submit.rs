use std::fmt;
use std::process::Command;
use std::time::Duration;

use crate::report::Report;

#[derive(Debug)]
pub enum SubmitError {
    /// Transient (timeout, sequence conflict): safe to retry.
    Transient(String),
    /// The contract refused the report: do not retry.
    Rejected(String),
}

impl fmt::Display for SubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubmitError::Transient(s) => write!(f, "transient: {s}"),
            SubmitError::Rejected(s) => write!(f, "rejected: {s}"),
        }
    }
}

/// Submission client; tests substitute a mock RPC.
pub trait Submitter {
    fn submit(&self, report: &Report) -> Result<(), SubmitError>;
}

/// Retries `Transient` errors up to `max_retries` times with exponential
/// backoff (`base_ms * 2^attempt`, capped at 60s). `Rejected` returns
/// immediately.
pub fn submit_with_retry(
    s: &dyn Submitter,
    report: &Report,
    max_retries: u32,
    base_ms: u64,
    sleep: &dyn Fn(Duration),
) -> Result<(), SubmitError> {
    let mut attempt = 0;
    loop {
        match s.submit(report) {
            Err(SubmitError::Transient(e)) if attempt < max_retries => {
                let ms = base_ms.saturating_mul(1u64 << attempt.min(20)).min(60_000);
                sleep(Duration::from_millis(ms));
                attempt += 1;
                let _ = e;
            }
            other => return other,
        }
    }
}

/// Submits by shelling out to the `stellar` CLI, which builds, signs and
/// sends the transaction. The secret is handed over through the child's
/// environment only — never argv, never logs.
///
/// Targets `submit_price(symbol, price, decimals)` on the oracle; adjust
/// `function` if the oracle entrypoint is renamed.
pub struct StellarCliSubmitter {
    pub rpc_url: String,
    pub network_passphrase: String,
    pub contract_id: String,
    pub function: String,
    secret: String,
}

impl StellarCliSubmitter {
    pub fn new(rpc_url: &str, passphrase: &str, contract_id: &str, secret: String) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            network_passphrase: passphrase.into(),
            contract_id: contract_id.into(),
            function: "submit_price".into(),
            secret,
        }
    }
}

impl Submitter for StellarCliSubmitter {
    fn submit(&self, r: &Report) -> Result<(), SubmitError> {
        let out = Command::new("stellar")
            .args(["contract", "invoke", "--id", &self.contract_id])
            .args(["--rpc-url", &self.rpc_url])
            .args(["--network-passphrase", &self.network_passphrase])
            .args(["--source-account", "feeder"])
            .env("STELLAR_SECRET_KEY", &self.secret)
            .args(["--", &self.function])
            .args(["--symbol", &r.symbol])
            .args(["--price", &r.price.to_string()])
            .args(["--decimals", &r.decimals.to_string()])
            .output()
            .map_err(|e| SubmitError::Transient(e.to_string()))?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr).replace(&self.secret, "[redacted]");
        if err.contains("Error(Contract") {
            Err(SubmitError::Rejected(err))
        } else {
            Err(SubmitError::Transient(err))
        }
    }
}
