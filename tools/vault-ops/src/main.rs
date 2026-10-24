//! Read-only vault operations: `tags`, `reconcile`, `audit-tag`.
//!
//! All chain access goes through the [`Ledger`] trait. The real
//! implementation shells out to `stellar contract invoke --send no`
//! (simulation only) and `stellar events`, so no signing key, write path or
//! admin credential is ever involved. Tests drive the same logic through a
//! mock `Ledger`.

use serde_json::{json, Value};
use std::process::{exit, Command};

type Result<T> = std::result::Result<T, String>;

/// Page size for `get_tags`; the vault caps `limit` at 100.
const PAGE: u32 = 100;

pub trait Ledger {
    /// Simulates `contract.func(--name value ...)` and returns its JSON result.
    fn invoke(&self, contract: &str, func: &str, args: &[(&str, String)]) -> Result<Value>;
    /// Raw JSON events emitted by `contract`, oldest first.
    fn events(&self, contract: &str, start_ledger: u64) -> Result<Vec<Value>>;
}

/// `stellar` CLI backed ledger. Simulation only (`--send no`).
pub struct StellarCli {
    pub network: String,
    pub source: String,
}

impl Ledger for StellarCli {
    fn invoke(&self, contract: &str, func: &str, args: &[(&str, String)]) -> Result<Value> {
        let mut cmd = Command::new("stellar");
        cmd.args([
            "contract",
            "invoke",
            "--id",
            contract,
            "--network",
            &self.network,
        ]);
        cmd.args(["--source-account", &self.source, "--send", "no", "--", func]);
        for (name, value) in args {
            cmd.arg(format!("--{name}")).arg(value);
        }
        let out = cmd
            .output()
            .map_err(|e| format!("can't run `stellar`: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "{contract}.{func} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("{contract}.{func}: unparseable output: {e}"))
    }

    fn events(&self, contract: &str, start_ledger: u64) -> Result<Vec<Value>> {
        let out = Command::new("stellar")
            .args(["events", "--id", contract, "--network", &self.network])
            .args([
                "--start-ledger",
                &start_ledger.to_string(),
                "--output",
                "json",
            ])
            .output()
            .map_err(|e| format!("can't run `stellar`: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "events for {contract} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        // One JSON document per line, or a single array.
        let text = String::from_utf8_lossy(&out.stdout);
        if let Ok(Value::Array(v)) = serde_json::from_str::<Value>(&text) {
            return Ok(v);
        }
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(|e| format!("bad event line: {e}")))
            .collect()
    }
}

/// i128/u64 values arrive as JSON numbers or decimal strings.
fn int(v: &Value) -> Result<i128> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .map(i128::from)
            .ok_or_else(|| format!("not an integer: {v}")),
        Value::String(s) => s.trim().parse().map_err(|_| format!("not an integer: {v}")),
        Value::Object(o) if o.len() == 1 => int(o.values().next().unwrap()),
        _ => Err(format!("not an integer: {v}")),
    }
}

/// Strips the `{"symbol": "x"}` / `{"address": "G..."}` wrappers some CLI
/// versions add around scalar values.
fn plain(v: &Value) -> Value {
    match v {
        Value::Object(o) if o.len() == 1 => {
            let (k, inner) = o.iter().next().unwrap();
            if matches!(
                k.as_str(),
                "symbol" | "address" | "string" | "u64" | "i128" | "u32"
            ) {
                return plain(inner);
            }
            v.clone()
        }
        _ => v.clone(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TagBalance {
    pub tag: Value,
    pub balance: i128,
}

pub fn tag_label(tag: &Value) -> String {
    let f = |k: &str| plain(tag.get(k).unwrap_or(&Value::Null));
    format!(
        "{}:{}:{}",
        f("owner").as_str().unwrap_or("?"),
        f("kind").as_str().unwrap_or("?"),
        f("id")
    )
}

/// Every tag the vault indexes, via `get_tags` pages. Fails rather than
/// truncating: the page walk must reach `get_tag_count` and collect exactly
/// that many tags.
pub fn all_tags(l: &dyn Ledger, vault: &str) -> Result<Vec<Value>> {
    let count = int(&l.invoke(vault, "get_tag_count", &[])?)? as u32;
    let (mut cursor, mut tags) = (0u32, Vec::new());
    while cursor < count {
        let page = l.invoke(
            vault,
            "get_tags",
            &[("cursor", cursor.to_string()), ("limit", PAGE.to_string())],
        )?;
        let (items, next) = match &page {
            Value::Array(p) if p.len() == 2 && p[0].is_array() => {
                (p[0].as_array().unwrap().clone(), int(&p[1])? as u32)
            }
            _ => return Err(format!("get_tags: unexpected page shape: {page}")),
        };
        if next <= cursor && next < count {
            return Err(format!("get_tags: cursor stuck at {cursor}"));
        }
        tags.extend(items);
        cursor = next;
    }
    if tags.len() != count as usize {
        return Err(format!(
            "get_tags returned {} tags but get_tag_count is {count} (ledger changed mid-read? retry)",
            tags.len()
        ));
    }
    Ok(tags)
}

fn balance_of(l: &dyn Ledger, vault: &str, token: &str, tag: &Value) -> Result<i128> {
    int(&l.invoke(
        vault,
        "balance_of",
        &[("token", token.to_string()), ("tag", tag.to_string())],
    )?)
}

/// Live (nonzero-balance) tags for `token`.
pub fn live_tags(l: &dyn Ledger, vault: &str, token: &str) -> Result<Vec<TagBalance>> {
    let mut out = Vec::new();
    for tag in all_tags(l, vault)? {
        let balance = balance_of(l, vault, token, &tag)?;
        if balance != 0 {
            out.push(TagBalance { tag, balance });
        }
    }
    Ok(out)
}

#[derive(Debug, PartialEq)]
pub struct Reconciliation {
    pub tags: usize,
    pub recorded_sum: i128,
    pub total_escrowed: i128,
    pub actual_balance: i128,
    pub problems: Vec<String>,
}

pub fn reconcile(l: &dyn Ledger, vault: &str, token: &str) -> Result<Reconciliation> {
    let tags = live_tags(l, vault, token)?;
    let recorded_sum: i128 = tags.iter().map(|t| t.balance).sum();
    let total_escrowed =
        int(&l.invoke(vault, "get_total_escrowed", &[("token", token.to_string())])?)?;
    let actual_balance = int(&l.invoke(token, "balance", &[("id", vault.to_string())])?)?;
    let mut problems = Vec::new();
    if recorded_sum != total_escrowed {
        problems.push(format!(
            "tag balances sum to {recorded_sum} but get_total_escrowed is {total_escrowed}"
        ));
    }
    if recorded_sum > actual_balance {
        problems.push(format!(
            "shortfall: recorded {recorded_sum} exceeds actual token balance {actual_balance}"
        ));
    } else if recorded_sum < actual_balance {
        problems.push(format!(
            "untagged funds: actual token balance {actual_balance} exceeds recorded {recorded_sum}"
        ));
    }
    Ok(Reconciliation {
        tags: tags.len(),
        recorded_sum,
        total_escrowed,
        actual_balance,
        problems,
    })
}

#[derive(Debug, PartialEq)]
pub struct Audit {
    pub deposits: usize,
    pub deposited: i128,
    pub withdrawals: usize,
    pub withdrawn: i128,
}

/// Summarises `deposited`/`withdrawn` events for one `(token, tag)`.
pub fn audit_tag(
    l: &dyn Ledger,
    vault: &str,
    token: &str,
    tag: &Value,
    start: u64,
) -> Result<Audit> {
    let mut a = Audit {
        deposits: 0,
        deposited: 0,
        withdrawals: 0,
        withdrawn: 0,
    };
    for ev in l.events(vault, start)? {
        let topics: Vec<Value> = ev["topic"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(plain)
            .collect();
        if topics.len() < 3 || topics[1].as_str() != Some(token) || !same_tag(&topics[2], tag) {
            continue;
        }
        let value = &ev["value"];
        let amount = match value {
            Value::Array(v) if v.len() == 2 => int(&v[1])?,
            _ => return Err(format!("unexpected event value: {value}")),
        };
        match topics[0].as_str() {
            Some("deposited") => {
                a.deposits += 1;
                a.deposited += amount;
            }
            Some("withdrawn") => {
                a.withdrawals += 1;
                a.withdrawn += amount;
            }
            _ => {}
        }
    }
    Ok(a)
}

fn same_tag(a: &Value, b: &Value) -> bool {
    ["owner", "kind", "id"]
        .iter()
        .all(|k| plain(&a[k]) == plain(&b[k]))
}

/// `kind:id` -> a tag struct owned by `owner`.
pub fn parse_tag(owner: &str, spec: &str) -> Result<Value> {
    let (kind, id) = spec
        .split_once(':')
        .ok_or("tag must be <kind>:<id>, e.g. series:7")?;
    let id: u64 = id.parse().map_err(|_| format!("bad tag id `{id}`"))?;
    if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("bad tag kind `{kind}`"));
    }
    Ok(json!({ "owner": owner, "kind": kind, "id": id }))
}

const USAGE: &str = "\
usage: vault-ops <command> --vault <C...> [--network <name>] [--source <G...>] [--json]

commands:
  tags <token>                           live tags and balances
  reconcile <token>                      recorded sum vs actual balance; exit 1 on mismatch
  audit-tag <token> <integrator> <kind>:<id> [--start-ledger N]
                                         deposit/withdrawal summary from events

--source is any public account (used only to simulate; never signs).
Exit codes: 0 ok, 1 mismatch found, 2 error.";

fn run(args: &[String], l: &dyn Ledger, vault: &str, as_json: bool, start: u64) -> Result<i32> {
    let a = |i: usize| {
        args.get(i)
            .map(String::as_str)
            .ok_or_else(|| USAGE.to_string())
    };
    match a(0)? {
        "tags" => {
            let token = a(1)?;
            let tags = live_tags(l, vault, token)?;
            if as_json {
                let items: Vec<Value> = tags
                    .iter()
                    .map(|t| json!({ "tag": t.tag, "balance": t.balance.to_string() }))
                    .collect();
                println!("{}", json!({ "token": token, "tags": items }));
            } else {
                for t in &tags {
                    println!("{:<60} {}", tag_label(&t.tag), t.balance);
                }
                println!("{} live tags", tags.len());
            }
            Ok(0)
        }
        "reconcile" => {
            let r = reconcile(l, vault, a(1)?)?;
            if as_json {
                println!(
                    "{}",
                    json!({
                        "ok": r.problems.is_empty(),
                        "tags": r.tags,
                        "recorded_sum": r.recorded_sum.to_string(),
                        "total_escrowed": r.total_escrowed.to_string(),
                        "actual_balance": r.actual_balance.to_string(),
                        "problems": r.problems,
                    })
                );
            } else {
                println!(
                    "tags: {}  recorded: {}  total_escrowed: {}  actual: {}",
                    r.tags, r.recorded_sum, r.total_escrowed, r.actual_balance
                );
                for p in &r.problems {
                    println!("MISMATCH: {p}");
                }
                if r.problems.is_empty() {
                    println!("ledger OK");
                }
            }
            Ok(i32::from(!r.problems.is_empty()))
        }
        "audit-tag" => {
            let (token, integrator) = (a(1)?, a(2)?);
            let tag = parse_tag(integrator, a(3)?)?;
            let au = audit_tag(l, vault, token, &tag, start)?;
            if as_json {
                println!(
                    "{}",
                    json!({
                        "tag": tag, "deposits": au.deposits, "deposited": au.deposited.to_string(),
                        "withdrawals": au.withdrawals, "withdrawn": au.withdrawn.to_string(),
                        "net": (au.deposited - au.withdrawn).to_string(),
                    })
                );
            } else {
                println!(
                    "deposits: {} ({})  withdrawals: {} ({})  net: {}",
                    au.deposits,
                    au.deposited,
                    au.withdrawals,
                    au.withdrawn,
                    au.deposited - au.withdrawn
                );
            }
            Ok(0)
        }
        _ => Err(USAGE.to_string()),
    }
}

fn main() {
    let (mut pos, mut vault, mut network, mut source) =
        (Vec::new(), None, "testnet".to_string(), None);
    let (mut as_json, mut start) = (false, 1u64);
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || {
            it.next()
                .unwrap_or_else(|| fail(&format!("{arg} needs a value")))
        };
        match arg.as_str() {
            "--vault" => vault = Some(val()),
            "--network" => network = val(),
            "--source" => source = Some(val()),
            "--start-ledger" => {
                start = val().parse().unwrap_or_else(|_| fail("bad --start-ledger"))
            }
            "--json" => as_json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ => pos.push(arg),
        }
    }
    let vault = vault.unwrap_or_else(|| fail("--vault is required"));
    let source = source.unwrap_or_else(|| fail("--source <public account> is required"));
    let cli = StellarCli { network, source };
    match run(&pos, &cli, &vault, as_json, start) {
        Ok(code) => exit(code),
        Err(e) => fail(&e),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("vault-ops: {msg}");
    exit(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Mock {
        tags: Vec<Value>,
        balances: BTreeMap<usize, i128>,
        total: i128,
        actual: i128,
        events: Vec<Value>,
        fail_balance: bool,
    }

    fn tag(id: u64) -> Value {
        json!({ "owner": "CINT", "kind": "series", "id": id })
    }

    fn mock(n: usize, bal: i128) -> Mock {
        Mock {
            tags: (0..n as u64).map(tag).collect(),
            balances: (0..n).map(|i| (i, bal)).collect(),
            total: bal * n as i128,
            actual: bal * n as i128,
            events: vec![],
            fail_balance: false,
        }
    }

    impl Ledger for Mock {
        fn invoke(&self, _c: &str, func: &str, args: &[(&str, String)]) -> Result<Value> {
            let arg = |k: &str| args.iter().find(|(n, _)| *n == k).unwrap().1.clone();
            match func {
                "get_tag_count" => Ok(json!(self.tags.len())),
                "get_tags" => {
                    let (c, l): (usize, usize) = (
                        arg("cursor").parse().unwrap(),
                        arg("limit").parse().unwrap(),
                    );
                    assert!(l <= 100);
                    let end = (c + 3).min(self.tags.len()); // pages smaller than asked
                    Ok(json!([self.tags[c..end], end]))
                }
                "balance_of" => {
                    if self.fail_balance {
                        return Err("contract error".into());
                    }
                    let t: Value = serde_json::from_str(&arg("tag")).unwrap();
                    let i = self
                        .tags
                        .iter()
                        .position(|x| *x == t)
                        .ok_or("unknown tag")?;
                    Ok(json!(self.balances[&i].to_string()))
                }
                "get_total_escrowed" => Ok(json!(self.total.to_string())),
                "balance" => Ok(json!(self.actual.to_string())),
                f => Err(format!("unexpected {f}")),
            }
        }
        fn events(&self, _c: &str, _s: u64) -> Result<Vec<Value>> {
            Ok(self.events.clone())
        }
    }

    #[test]
    fn clean_reconcile() {
        let r = reconcile(&mock(7, 10), "V", "T").unwrap();
        assert!(r.problems.is_empty());
        assert_eq!((r.tags, r.recorded_sum), (7, 70));
    }

    #[test]
    fn mismatch_is_reported_with_nonzero_exit() {
        let mut m = mock(4, 10);
        m.actual = 25; // clawback
        let r = reconcile(&m, "V", "T").unwrap();
        assert!(r.problems.iter().any(|p| p.starts_with("shortfall")));
        assert_eq!(
            run(&["reconcile".into(), "T".into()], &m, "V", true, 1),
            Ok(1)
        );
        m.actual = 40;
        m.total = 41;
        assert_eq!(reconcile(&m, "V", "T").unwrap().problems.len(), 1);
        m.total = 40;
        m.actual = 50;
        assert!(reconcile(&m, "V", "T").unwrap().problems[0].starts_with("untagged"));
        assert_eq!(
            run(
                &["reconcile".into(), "T".into()],
                &mock(2, 5),
                "V",
                false,
                1
            ),
            Ok(0)
        );
    }

    #[test]
    fn pagination_is_complete_across_pages() {
        let m = mock(250, 1); // 84 pages of 3
        assert_eq!(all_tags(&m, "V").unwrap().len(), 250);
        assert_eq!(live_tags(&m, "V", "T").unwrap().len(), 250);
    }

    #[test]
    fn zero_balance_tags_are_not_live() {
        let mut m = mock(3, 10);
        m.balances.insert(1, 0);
        assert_eq!(live_tags(&m, "V", "T").unwrap().len(), 2);
    }

    #[test]
    fn contract_error_is_an_error_not_a_zero_balance() {
        let mut m = mock(2, 10);
        m.fail_balance = true;
        assert!(reconcile(&m, "V", "T").is_err());
        assert_eq!(
            run(&["reconcile".into(), "T".into()], &m, "V", false, 1),
            Err("contract error".into())
        );
    }

    #[test]
    fn truncated_tag_listing_is_an_error() {
        struct Short(Mock);
        impl Ledger for Short {
            fn invoke(&self, c: &str, f: &str, a: &[(&str, String)]) -> Result<Value> {
                if f == "get_tag_count" {
                    return Ok(json!(10));
                }
                self.0.invoke(c, f, a)
            }
            fn events(&self, c: &str, s: u64) -> Result<Vec<Value>> {
                self.0.events(c, s)
            }
        }
        assert!(all_tags(&Short(mock(4, 1)), "V").is_err());
    }

    #[test]
    fn malformed_tag_and_usage_inputs() {
        assert!(parse_tag("CINT", "series").is_err());
        assert!(parse_tag("CINT", "series:x").is_err());
        assert!(parse_tag("CINT", "bad kind:1").is_err());
        assert_eq!(parse_tag("CINT", "series:7").unwrap(), tag(7));
        assert!(run(&["bogus".into()], &mock(1, 1), "V", false, 1).is_err());
        assert!(run(&["tags".into()], &mock(1, 1), "V", false, 1).is_err());
    }

    #[test]
    fn audit_sums_only_the_requested_token_and_tag() {
        let ev = |k: &str, token: &str, t: Value, amt: i64| json!({ "topic": [{"symbol": k}, token, t], "value": ["GADDR", amt.to_string()] });
        let mut m = mock(2, 1);
        m.events = vec![
            ev("deposited", "T", tag(7), 100),
            ev("deposited", "T", tag(7), 50),
            ev("withdrawn", "T", tag(7), 30),
            ev("deposited", "T", tag(8), 999),
            ev("deposited", "OTHER", tag(7), 999),
        ];
        let a = audit_tag(&m, "V", "T", &tag(7), 1).unwrap();
        assert_eq!(
            a,
            Audit {
                deposits: 2,
                deposited: 150,
                withdrawals: 1,
                withdrawn: 30
            }
        );
    }
}
