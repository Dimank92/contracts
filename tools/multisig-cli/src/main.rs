//! `multisig-cli` — see tools/multisig-cli/README.md.

use multisig_cli::{collect, payload, status, stellar::StellarCli, submit, Action, Approver};
use serde_json::{Map, Value};
use std::{collections::HashMap, fs, process::exit};

const USAGE: &str = "usage: multisig-cli <payload|status|collect|submit> [options]

common:   --multisig <C...>  --rpc-url <url>  --network-passphrase <s>  [--json]
payload:  --target <C...> --function <name> [--args '{\"p\":\"v\"}'] [--signers G1,G2]
status:   --action-id <hex> [--class 0|1|2]
collect:  --action-id <hex> (--signer <identity> | --signed <xdr-file>)... [--dry-run]
submit:   --action-id <hex> --target <C...> --function <name> [--args ...]
          --source <identity> [--class N] [--dry-run]";

type Parsed = (HashMap<String, Vec<String>>, Vec<String>);

fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut opts: HashMap<String, Vec<String>> = HashMap::new();
    let mut flags = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let name = a
            .strip_prefix("--")
            .ok_or(format!("unexpected argument `{a}`"))?;
        if matches!(name, "json" | "dry-run") {
            flags.push(name.to_string());
        } else {
            let v = it.next().ok_or(format!("--{name} needs a value"))?;
            opts.entry(name.to_string()).or_default().push(v.clone());
        }
    }
    Ok((opts, flags))
}

fn action<'a>(target: &'a str, function: &'a str, args: &'a Map<String, Value>) -> Action<'a> {
    Action {
        target,
        function,
        args,
    }
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cmd = argv.first().ok_or(USAGE)?.as_str();
    let (o, flags) = parse(&argv[1..])?;
    let get = |k: &str| o.get(k).and_then(|v| v.first()).cloned();
    let need = |k: &str| get(k).ok_or(format!("--{k} is required\n\n{USAGE}"));
    let (dry_run, json) = (
        flags.iter().any(|f| f == "dry-run"),
        flags.iter().any(|f| f == "json"),
    );
    let multisig = need("multisig")?;
    let rpc_url = get("rpc-url").unwrap_or_else(|| "https://soroban-testnet.stellar.org".into());
    let passphrase =
        get("network-passphrase").unwrap_or_else(|| "Test SDF Network ; September 2015".into());
    let cli = StellarCli {
        rpc_url: rpc_url.clone(),
        network_passphrase: passphrase.clone(),
    };
    let class = get("class")
        .map(|c| c.parse::<u32>().map_err(|e| format!("--class: {e}")))
        .transpose()?;

    let args: Map<String, Value> = match get("args") {
        Some(s) => {
            serde_json::from_str(&s).map_err(|e| format!("--args must be a JSON object: {e}"))?
        }
        None => Map::new(),
    };
    let out = match cmd {
        "payload" => {
            let (t, f) = (need("target")?, need("function")?);
            let signers: Vec<String> = get("signers")
                .map(|s| s.split(',').map(str::to_string).collect())
                .unwrap_or_default();
            let net = format!("--rpc-url {rpc_url} --network-passphrase '{passphrase}'");
            payload(&net, &multisig, &action(&t, &f, &args), &signers)
        }
        "status" => status(&cli, &multisig, &need("action-id")?, class)?,
        "collect" => {
            let mut approvers: Vec<Approver> = o
                .get("signer")
                .into_iter()
                .flatten()
                .cloned()
                .map(Approver::Identity)
                .collect();
            for path in o.get("signed").into_iter().flatten() {
                approvers.push(Approver::Signed(
                    fs::read_to_string(path)
                        .map_err(|e| format!("{path}: {e}"))?
                        .trim()
                        .to_string(),
                ));
            }
            collect(&cli, &multisig, &need("action-id")?, &approvers, dry_run)?
        }
        "submit" => {
            let (t, f) = (need("target")?, need("function")?);
            submit(
                &cli,
                &multisig,
                &need("action-id")?,
                &action(&t, &f, &args),
                class,
                &need("source")?,
                dry_run,
            )?
        }
        _ => return Err(USAGE.into()),
    };
    if json {
        println!("{out}");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("multisig-cli: {e}");
        exit(1);
    }
}
