//! [`Backend`] that shells out to `stellar-cli` (`stellar contract invoke`,
//! `stellar tx send`, `stellar keys address`).

use crate::{Args, Backend, Result};
use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Stdio},
};

pub struct StellarCli {
    pub rpc_url: String,
    pub network_passphrase: String,
}

fn run(mut cmd: Command, stdin: Option<&str>) -> Result<String> {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run stellar-cli: {e}"))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(input.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

impl StellarCli {
    fn invoke(
        &self,
        contract: &str,
        func: &str,
        args: Args,
        send: bool,
        source: Option<&str>,
    ) -> Result<String> {
        let mut cmd = Command::new("stellar");
        cmd.args([
            "contract",
            "invoke",
            "--id",
            contract,
            "--rpc-url",
            &self.rpc_url,
        ]);
        cmd.args(["--network-passphrase", &self.network_passphrase]);
        cmd.args(["--send", if send { "yes" } else { "no" }]);
        // Read-only simulation still needs a source account to build the tx.
        cmd.args(["--source", source.unwrap_or("default")]);
        cmd.args(["--", func]);
        for (name, value) in args {
            cmd.arg(format!("--{name}")).arg(value);
        }
        run(cmd, None)
    }
}

impl Backend for StellarCli {
    fn read(&self, contract: &str, func: &str, args: Args) -> Result<Value> {
        let out = self.invoke(contract, func, args, false, None)?;
        serde_json::from_str(&out).map_err(|e| format!("{func}: unparsable output {out:?}: {e}"))
    }

    fn send(&self, contract: &str, func: &str, args: Args, source: &str) -> Result<String> {
        self.invoke(contract, func, args, true, Some(source))
    }

    fn send_signed(&self, xdr: &str) -> Result<String> {
        let mut cmd = Command::new("stellar");
        cmd.args([
            "tx",
            "send",
            "--rpc-url",
            &self.rpc_url,
            "--network-passphrase",
            &self.network_passphrase,
        ]);
        run(cmd, Some(xdr))
    }

    fn address_of(&self, identity: &str) -> Result<String> {
        let mut cmd = Command::new("stellar");
        cmd.args(["keys", "address", identity]);
        run(cmd, None)
    }
}
