use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// Last accepted report is older than the warn threshold.
    Warn,
    /// Stale beyond the oracle window, or the contract rejected us:
    /// exit non-zero so the supervisor alerts/restarts.
    Exit(String),
}

#[derive(Default)]
pub struct Monitor {
    last_accepted: HashMap<String, u64>,
    rejected: bool,
    started_at: u64,
}

impl Monitor {
    pub fn new(started_at: u64) -> Self {
        Monitor {
            started_at,
            ..Default::default()
        }
    }

    pub fn record_accepted(&mut self, symbol: &str, now: u64) {
        self.last_accepted.insert(symbol.to_string(), now);
    }

    /// A contract rejection (outlier/quorum rules) is exit-worthy: retrying
    /// the same value will not help.
    pub fn record_rejected(&mut self, _symbol: &str) {
        self.rejected = true;
    }

    pub fn check(&self, now: u64, window: u64, warn: u64) -> Status {
        if self.rejected {
            return Status::Exit("report rejected by contract".into());
        }
        let oldest = self
            .last_accepted
            .values()
            .map(|t| now.saturating_sub(*t))
            .max()
            .unwrap_or_else(|| now.saturating_sub(self.started_at));
        if oldest > window {
            Status::Exit(format!(
                "last accepted report is {oldest}s old (window {window}s)"
            ))
        } else if oldest > warn {
            Status::Warn
        } else {
            Status::Ok
        }
    }
}
