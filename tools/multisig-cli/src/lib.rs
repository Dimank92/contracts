//! Operational CLI logic for the `multisig` contract: build the payload to
//! approve, collect approvals, report status, and submit once approved.
//!
//! The contract is the only source of truth for threshold, weights and
//! expiry: every decision here goes through its own `is_approved` /
//! `is_executable` — nothing re-implements the threshold check. All network
//! access goes through [`Backend`], so tests swap in the Soroban test env.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub mod stellar;

pub type Result<T> = std::result::Result<T, String>;
/// Named contract-call arguments, in call order.
pub type Args<'a> = &'a [(&'a str, String)];

/// Network layer. `read` simulates (no fee, no state change); `send`
/// signs with the named `stellar-cli` identity and submits.
pub trait Backend {
    fn read(&self, contract: &str, func: &str, args: Args) -> Result<Value>;
    fn send(&self, contract: &str, func: &str, args: Args, source: &str) -> Result<String>;
    /// Submits an already-signed transaction envelope (base64 XDR).
    fn send_signed(&self, xdr: &str) -> Result<String>;
    /// Public address (G...) of a `stellar-cli` identity.
    fn address_of(&self, identity: &str) -> Result<String>;
}

/// Who contributes an approval.
pub enum Approver {
    /// A local `stellar-cli` identity that signs and submits `approve`.
    Identity(String),
    /// A signed `approve` transaction produced offline (base64 XDR).
    Signed(String),
}

/// The call a multisig action authorizes.
pub struct Action<'a> {
    pub target: &'a str,
    pub function: &'a str,
    pub args: &'a Map<String, Value>,
}

/// Stable id for an action: `sha256` of its canonical JSON (keys sorted),
/// hex-encoded. Deriving the id from the payload means one `action_id` can
/// never be reused for a different payload.
pub fn action_id(a: &Action) -> String {
    let canonical = json!({ "target": a.target, "function": a.function, "args": a.args });
    let digest = Sha256::digest(canonical.to_string().as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The exact payload signers approve, plus the per-signer unsigned-tx
/// command for offline signing. Needs no network.
pub fn payload(network_args: &str, multisig: &str, a: &Action, signers: &[String]) -> Value {
    let id = action_id(a);
    let approve: Vec<Value> = signers
        .iter()
        .map(|s| {
            json!({
                "signer": s,
                "command": format!(
                    "stellar contract invoke --id {multisig} {network_args} --source {s} --build-only -- approve --signer {s} --action_id {id}"
                ),
            })
        })
        .collect();
    json!({
        "multisig": multisig,
        "action_id": id,
        "target": a.target,
        "function": a.function,
        "args": a.args,
        "approve": approve,
    })
}

fn weights(v: &Value) -> Result<Vec<(String, u64)>> {
    v.as_array()
        .ok_or("get_signers: expected a map")?
        .iter()
        .map(|e| {
            let addr = e["key"].as_str().ok_or("get_signers: bad key")?;
            Ok((
                addr.to_string(),
                e["val"].as_u64().ok_or("get_signers: bad weight")?,
            ))
        })
        .collect()
}

fn id_arg(id: &str) -> [(&'static str, String); 1] {
    [("action_id", id.to_string())]
}

fn flag(b: &dyn Backend, m: &str, f: &str, args: Args) -> Result<bool> {
    b.read(m, f, args)?
        .as_bool()
        .ok_or_else(|| format!("{f}: expected a bool"))
}

/// Approval status for `action_id`, as reported by the contract.
pub fn status(b: &dyn Backend, m: &str, id: &str, class: Option<u32>) -> Result<Value> {
    let signers = weights(&b.read(m, "get_signers", &[])?)?;
    let mut rows = Vec::new();
    for (addr, w) in &signers {
        let args = [("action_id", id.to_string()), ("signer", addr.clone())];
        rows.push(
            json!({ "address": addr, "weight": w, "approved": flag(b, m, "has_approved", &args)? }),
        );
    }
    let executable = match class {
        Some(c) => Some(flag(
            b,
            m,
            "is_executable",
            &[("action_id", id.to_string()), ("class", c.to_string())],
        )?),
        None => None,
    };
    Ok(json!({
        "action_id": id,
        "approved": flag(b, m, "is_approved", &id_arg(id))?,
        "executable": executable,
        "approval_count": b.read(m, "get_approval_count", &id_arg(id))?,
        "approval_weight": b.read(m, "get_approval_weight", &id_arg(id))?,
        "threshold": b.read(m, "get_threshold", &[])?,
        "approval_ttl": b.read(m, "get_approval_ttl", &[])?,
        "execute_by": b.read(m, "get_execute_by", &id_arg(id))?,
        "signers": rows,
    }))
}

/// Submits each approver's `approve`, then reports the contract's verdict.
/// `dry_run` validates signer membership and submits nothing.
pub fn collect(
    b: &dyn Backend,
    m: &str,
    id: &str,
    approvers: &[Approver],
    dry_run: bool,
) -> Result<Value> {
    let signers = weights(&b.read(m, "get_signers", &[])?)?;
    let mut done = Vec::new();
    for a in approvers {
        match a {
            Approver::Identity(name) => {
                let addr = b.address_of(name)?;
                if !signers.iter().any(|(s, _)| *s == addr) {
                    return Err(format!(
                        "{name} ({addr}) is not a signer of {m} (signer set may have changed)"
                    ));
                }
                let args = [("signer", addr.clone()), ("action_id", id.to_string())];
                let tx = if dry_run {
                    "dry-run".into()
                } else {
                    b.send(m, "approve", &args, name)?
                };
                done.push(json!({ "signer": addr, "tx": tx }));
            }
            Approver::Signed(xdr) => {
                let tx = if dry_run {
                    "dry-run".into()
                } else {
                    b.send_signed(xdr)?
                };
                done.push(json!({ "signer": null, "tx": tx }));
            }
        }
    }
    Ok(
        json!({ "action_id": id, "dry_run": dry_run, "submitted": done, "approved": flag(b, m, "is_approved", &id_arg(id))? }),
    )
}

/// Sends the approved action. Refuses unless the contract says it is
/// approved (and executable for `class`, when given), checked right before
/// sending so a changed signer set or an expired approval is caught.
pub fn submit(
    b: &dyn Backend,
    m: &str,
    id: &str,
    a: &Action,
    class: Option<u32>,
    source: &str,
    dry_run: bool,
) -> Result<Value> {
    if action_id(a) != id {
        return Err(format!(
            "action_id {id} does not match the payload (expected {})",
            action_id(a)
        ));
    }
    let st = status(b, m, id, class)?;
    if st["approved"] != true {
        return Err(format!(
            "not approved: weight {} of threshold {} (approvals may have expired or the signer set changed)",
            st["approval_weight"], st["threshold"]
        ));
    }
    if class.is_some() && st["executable"] != true {
        return Err("approved but not executable for this class yet".into());
    }
    let args: Vec<(&str, String)> = a
        .args
        .iter()
        .map(|(k, v)| {
            (
                k.as_str(),
                v.as_str().map_or_else(|| v.to_string(), str::to_string),
            )
        })
        .collect();
    let tx = if dry_run {
        "dry-run".to_string()
    } else {
        b.send(a.target, a.function, &args, source)?
    };
    Ok(json!({ "action_id": id, "dry_run": dry_run, "tx": tx }))
}
