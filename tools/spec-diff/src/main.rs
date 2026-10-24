//! Compares the contract spec (public ABI) embedded in two builds of the
//! same contract and fails if anything in `base` was removed or changed
//! in `head`. Additions (new functions, new error/enum/union cases) are
//! allowed and listed. Functions whose signature is intentionally
//! changed can be exempted with `--allow <name>`.
//!
//! `--allow '*'` allows every function's signature to change (functions
//! still can't be removed), for a PR that deliberately changes a whole
//! contract's ABI.
//!
//! Usage: spec-diff <base.wasm> <head.wasm> [--allow <fn>]...

use std::{collections::BTreeMap, process::exit};
use stellar_xdr::curr::{ScSpecEntry, StringM};

fn name(e: &ScSpecEntry) -> String {
    let n = match e {
        ScSpecEntry::FunctionV0(x) => x.name.0.to_utf8_string_lossy(),
        ScSpecEntry::UdtStructV0(x) => x.name.to_utf8_string_lossy(),
        ScSpecEntry::UdtUnionV0(x) => x.name.to_utf8_string_lossy(),
        ScSpecEntry::UdtEnumV0(x) => x.name.to_utf8_string_lossy(),
        ScSpecEntry::UdtErrorEnumV0(x) => x.name.to_utf8_string_lossy(),
    };
    format!("{}:{n}", e.name())
}

/// Strips doc strings so only the ABI-relevant shape is compared.
fn strip_docs(mut e: ScSpecEntry) -> ScSpecEntry {
    let empty = || StringM::default();
    match &mut e {
        ScSpecEntry::FunctionV0(x) => {
            x.doc = empty();
            let mut inputs = x.inputs.to_vec();
            inputs.iter_mut().for_each(|i| i.doc = empty());
            x.inputs = inputs.try_into().unwrap();
        }
        ScSpecEntry::UdtStructV0(x) => {
            x.doc = empty();
            let mut f = x.fields.to_vec();
            f.iter_mut().for_each(|i| i.doc = empty());
            x.fields = f.try_into().unwrap();
        }
        ScSpecEntry::UdtUnionV0(x) => {
            x.doc = empty();
            let mut c = x.cases.to_vec();
            c.iter_mut().for_each(|c| match c {
                stellar_xdr::curr::ScSpecUdtUnionCaseV0::VoidV0(v) => v.doc = empty(),
                stellar_xdr::curr::ScSpecUdtUnionCaseV0::TupleV0(t) => t.doc = empty(),
            });
            x.cases = c.try_into().unwrap();
        }
        ScSpecEntry::UdtEnumV0(x) => {
            x.doc = empty();
            let mut c = x.cases.to_vec();
            c.iter_mut().for_each(|c| c.doc = empty());
            x.cases = c.try_into().unwrap();
        }
        ScSpecEntry::UdtErrorEnumV0(x) => {
            x.doc = empty();
            let mut c = x.cases.to_vec();
            c.iter_mut().for_each(|c| c.doc = empty());
            x.cases = c.try_into().unwrap();
        }
    }
    e
}

/// True if every case of `base` still exists, unchanged, in `head`.
fn cases_superset<T: PartialEq>(base: &[T], head: &[T]) -> bool {
    base.iter().all(|c| head.contains(c))
}

fn compatible(base: &ScSpecEntry, head: &ScSpecEntry) -> bool {
    match (base, head) {
        (ScSpecEntry::UdtUnionV0(b), ScSpecEntry::UdtUnionV0(h)) => {
            cases_superset(&b.cases, &h.cases)
        }
        (ScSpecEntry::UdtEnumV0(b), ScSpecEntry::UdtEnumV0(h)) => {
            cases_superset(&b.cases, &h.cases)
        }
        (ScSpecEntry::UdtErrorEnumV0(b), ScSpecEntry::UdtErrorEnumV0(h)) => {
            cases_superset(&b.cases, &h.cases)
        }
        _ => base == head,
    }
}

fn load(path: &str) -> BTreeMap<String, ScSpecEntry> {
    let wasm = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    soroban_spec::read::from_wasm(&wasm)
        .unwrap_or_else(|e| panic!("{path}: {e:?}"))
        .into_iter()
        .map(|e| (name(&e), strip_docs(e)))
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: spec-diff <base.wasm> <head.wasm> [--allow <fn>]...");
        exit(2);
    }
    let allowed: Vec<String> = args[2..]
        .chunks(2)
        .filter(|c| c[0] == "--allow")
        .map(|c| format!("FunctionV0:{}", c[1]))
        .collect();
    let (base, head) = (load(&args[0]), load(&args[1]));

    let mut failed = false;
    for (k, b) in &base {
        match head.get(k) {
            None => {
                println!("REMOVED  {k}");
                failed = true;
            }
            Some(h) if !compatible(b, h) => {
                let any_fn =
                    k.starts_with("FunctionV0:") && allowed.iter().any(|a| a == "FunctionV0:*");
                if any_fn || allowed.contains(k) {
                    println!("changed  {k} (allowed)");
                } else {
                    println!("CHANGED  {k}");
                    failed = true;
                }
            }
            Some(h) if b != h => println!("extended {k}"),
            Some(_) => {}
        }
    }
    for k in head.keys().filter(|k| !base.contains_key(*k)) {
        println!("added    {k}");
    }
    if failed {
        println!("spec-diff: existing ABI changed");
        exit(1);
    }
    println!("spec-diff: existing ABI preserved");
}
