//! Contract API diff: extracts the exported entrypoints of a built wasm
//! (names, parameter names/types, return types) from its embedded contract
//! spec, as stable sorted JSON, and diffs it against a committed baseline.
//!
//! ```text
//! api-diff dump  <wasm>                          print the interface JSON
//! api-diff check --baselines <dir> <wasm>...     diff against <dir>/<stem>.json
//! api-diff update --baselines <dir> <wasm>...    (re)write the baselines
//! ```
//!
//! `check` fails on a removed entrypoint or a changed parameter/return type
//! (a rename is reported as a removal plus an addition: the data does not
//! say more). Additions are reported but pass. A wasm with no baseline is
//! reported as "no baseline" and passes, so a crate can be bootstrapped with
//! `update` in the same PR that adds it.

use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, fs, path::Path, process::exit};
use stellar_xdr::curr::{ScSpecEntry, ScSpecTypeDef as T};

fn ty(t: &T) -> String {
    match t {
        T::Option(o) => format!("Option<{}>", ty(&o.value_type)),
        T::Result(r) => format!("Result<{}, {}>", ty(&r.ok_type), ty(&r.error_type)),
        T::Vec(v) => format!("Vec<{}>", ty(&v.element_type)),
        T::Map(m) => format!("Map<{}, {}>", ty(&m.key_type), ty(&m.value_type)),
        T::Tuple(t) => format!(
            "({})",
            t.value_types.iter().map(ty).collect::<Vec<_>>().join(", ")
        ),
        T::BytesN(n) => format!("BytesN<{}>", n.n),
        T::Udt(u) => u.name.to_utf8_string_lossy(),
        other => format!("{other:?}"),
    }
}

/// `{ "<fn>": { "inputs": [[name, type], ...], "outputs": [type, ...] } }`,
/// keyed by name so serialization is sorted and deterministic.
fn interface(wasm: &[u8]) -> Result<Value, String> {
    let entries = soroban_spec::read::from_wasm(wasm).map_err(|e| format!("{e:?}"))?;
    let mut fns = BTreeMap::new();
    for e in entries {
        if let ScSpecEntry::FunctionV0(f) = e {
            let inputs: Vec<Value> = f
                .inputs
                .iter()
                .map(|i| json!([i.name.to_utf8_string_lossy(), ty(&i.type_)]))
                .collect();
            let outputs: Vec<String> = f.outputs.iter().map(ty).collect();
            fns.insert(
                f.name.0.to_utf8_string_lossy(),
                json!({ "inputs": inputs, "outputs": outputs }),
            );
        }
    }
    Ok(json!({ "functions": Value::Object(fns.into_iter().collect::<Map<_, _>>()) }))
}

fn render(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default() + "\n"
}

/// Returns (breaking, additions) as human-readable lines.
fn diff(base: &Value, head: &Value) -> (Vec<String>, Vec<String>) {
    let empty = Map::new();
    let get = |v: &'_ Value| v["functions"].as_object().unwrap_or(&empty).clone();
    let (b, h) = (get(base), get(head));
    let mut breaking = Vec::new();
    for (name, sig) in &b {
        match h.get(name) {
            None => breaking.push(format!("REMOVED  {name}")),
            Some(new) if new != sig => breaking.push(format!(
                "CHANGED  {name}: {} -> {}",
                describe(sig),
                describe(new)
            )),
            Some(_) => {}
        }
    }
    let added = h
        .keys()
        .filter(|k| !b.contains_key(*k))
        .map(|k| format!("added    {k}: {}", describe(&h[k])))
        .collect();
    (breaking, added)
}

fn describe(sig: &Value) -> String {
    let params: Vec<String> = sig["inputs"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| {
                    format!(
                        "{}: {}",
                        p[0].as_str().unwrap_or("?"),
                        p[1].as_str().unwrap_or("?")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let outs: Vec<&str> = sig["outputs"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    format!(
        "({}) -> {}",
        params.join(", "),
        if outs.is_empty() {
            "()".into()
        } else {
            outs.join(", ")
        }
    )
}

fn load(wasm: &str) -> Result<Value, String> {
    let bytes = fs::read(wasm).map_err(|e| format!("{wasm}: {e}"))?;
    interface(&bytes).map_err(|e| format!("{wasm}: {e}"))
}

fn stem(wasm: &str) -> String {
    Path::new(wasm)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn run(args: &[String]) -> Result<bool, String> {
    let usage = "usage: api-diff dump <wasm> | check|update --baselines <dir> <wasm>...";
    match args.first().map(String::as_str) {
        Some("dump") if args.len() == 2 => {
            print!("{}", render(&load(&args[1])?));
            Ok(true)
        }
        Some(mode @ ("check" | "update"))
            if args.get(1).map(String::as_str) == Some("--baselines") && args.len() > 3 =>
        {
            let dir = Path::new(&args[2]);
            let mut ok = true;
            for wasm in &args[3..] {
                let head = load(wasm)?;
                let path = dir.join(format!("{}.json", stem(wasm)));
                if mode == "update" {
                    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                    fs::write(&path, render(&head))
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    println!("wrote    {}", path.display());
                    continue;
                }
                let Ok(text) = fs::read_to_string(&path) else {
                    println!(
                        "{}: no baseline at {} (first run); create one with `api-diff update`",
                        stem(wasm),
                        path.display()
                    );
                    continue;
                };
                let base: Value =
                    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                let (breaking, added) = diff(&base, &head);
                println!("== {}", stem(wasm));
                breaking.iter().chain(&added).for_each(|l| println!("{l}"));
                if !breaking.is_empty() {
                    println!("{}: breaking interface change (re-baseline with `api-diff update` if intended)", stem(wasm));
                    ok = false;
                }
            }
            Ok(ok)
        }
        _ => Err(usage.into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(true) => {}
        Ok(false) => exit(1),
        Err(e) => {
            eprintln!("api-diff: {e}");
            exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Sig = (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static [&'static str],
    );

    fn iface(fns: &[Sig]) -> Value {
        let m: Map<String, Value> = fns
            .iter()
            .map(|(n, i, o)| (n.to_string(), json!({ "inputs": i.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(), "outputs": o })))
            .collect();
        json!({ "functions": m })
    }

    const BASE: &[Sig] = &[("a", &[("x", "u32")], &["bool"]), ("b", &[], &[])];

    #[test]
    fn unchanged_and_reordered_only_pass() {
        let mut rev = BASE.to_vec();
        rev.reverse();
        let (br, add) = diff(&iface(BASE), &iface(&rev));
        assert!(br.is_empty() && add.is_empty());
        assert_eq!(
            render(&iface(BASE)),
            render(&iface(&rev)),
            "byte-identical output"
        );
    }

    #[test]
    fn addition_is_reported_not_breaking() {
        let mut more = BASE.to_vec();
        more.push(("c", &[], &[]));
        let (br, add) = diff(&iface(BASE), &iface(&more));
        assert!(br.is_empty());
        assert_eq!(add, vec!["added    c: () -> ()"]);
    }

    #[test]
    fn removal_and_rename_are_breaking() {
        let (br, _) = diff(&iface(BASE), &iface(&BASE[..1]));
        assert_eq!(br, vec!["REMOVED  b"]);
        let renamed = [("a2", BASE[0].1, BASE[0].2), BASE[1]];
        let (br, add) = diff(&iface(BASE), &iface(&renamed));
        assert_eq!(br, vec!["REMOVED  a"]);
        assert_eq!(add.len(), 1);
    }

    #[test]
    fn retyped_param_return_or_param_order_is_breaking() {
        for changed in [
            ("a", &[("x", "u64")][..], &["bool"][..]),
            ("a", &[("x", "u32")][..], &["u32"][..]),
            ("a", &[("y", "u32")][..], &["bool"][..]),
        ] {
            let (br, _) = diff(&iface(BASE), &iface(&[changed, BASE[1]]));
            assert_eq!(br.len(), 1, "{changed:?}");
        }
    }
}
