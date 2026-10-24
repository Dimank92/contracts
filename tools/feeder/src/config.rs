//! JSON config. The feeder secret is **never** part of the config file:
//! it is read from an env var or a keyfile that must not be group/world
//! accessible.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SymbolCfg {
    pub name: String,
    pub decimals: u32,
    /// Operator-supplied decimal price, e.g. "0.1234".
    pub price: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub rpc_url: String,
    pub network_passphrase: String,
    pub oracle_contract_id: String,
    pub symbols: Vec<SymbolCfg>,
    pub submit_interval_secs: u64,
    /// The oracle's configured staleness window.
    pub staleness_window_secs: u64,
    /// Warn once our last accepted report is this old (< window).
    pub staleness_warn_secs: u64,
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    #[serde(default = "default_backoff")]
    pub backoff_base_ms: u64,
    /// Name of the env var holding the secret key (default FEEDER_SECRET_KEY).
    #[serde(default = "default_env")]
    pub secret_env: String,
    /// Optional keyfile path used when the env var is unset.
    pub keyfile: Option<String>,
}

fn default_retries() -> u32 {
    4
}
fn default_backoff() -> u64 {
    500
}
fn default_env() -> String {
    "FEEDER_SECRET_KEY".into()
}

impl Config {
    pub fn parse(json: &str) -> Result<Config, String> {
        let cfg: Config = serde_json::from_str(json).map_err(|e| e.to_string())?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(self.rpc_url.starts_with("https://") || self.rpc_url.starts_with("http://")) {
            return Err("rpc_url must be an http(s) URL".into());
        }
        if self.symbols.is_empty() {
            return Err("symbols must not be empty".into());
        }
        if self.submit_interval_secs == 0 {
            return Err("submit_interval_secs must be > 0".into());
        }
        if self.staleness_warn_secs >= self.staleness_window_secs {
            return Err("staleness_warn_secs must be below staleness_window_secs".into());
        }
        if self.submit_interval_secs >= self.staleness_window_secs {
            return Err("submit_interval_secs must be below staleness_window_secs".into());
        }
        Ok(())
    }

    /// Loads the secret from the env var, else the keyfile (Unix mode must
    /// not allow group/other access). The value is returned, never logged.
    pub fn load_secret(&self) -> Result<String, String> {
        if let Ok(v) = std::env::var(&self.secret_env) {
            if !v.is_empty() {
                return Ok(v);
            }
        }
        let path = self
            .keyfile
            .as_ref()
            .ok_or_else(|| format!("{} unset and no keyfile configured", self.secret_env))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path)
                .map_err(|e| e.to_string())?
                .permissions()
                .mode();
            if mode & 0o077 != 0 {
                return Err(format!(
                    "keyfile {path} must not be group/world accessible (chmod 600)"
                ));
            }
        }
        std::fs::read_to_string(path)
            .map(|s| s.trim().to_string())
            .map_err(|e| e.to_string())
    }
}
