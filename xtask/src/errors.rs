//! `cargo xtask errors [--check]`: the cross-contract error registry
//! (docs/errors.md, #124), generated from each crate's `src/error.rs`.
//!
//! Every `#[contracterror]` variant must carry a doc comment whose first
//! paragraph says what the error means and whose `Cause:` line says what
//! typically triggers it:
//!
//! ```ignore
//! /// The contract has not been initialized.
//! /// Cause: calling an entrypoint before `initialize`.
//! NotInitialized = 12,
//! ```
//!
//! Codes from `SHARED_FROM` up are shared: the same code must have the
//! same name in every contract that defines it.

use std::collections::BTreeMap;
use std::fs;

use crate::source::read;
use crate::{Opts, Result, Workspace};

const REGISTRY: &str = "docs/errors.md";
/// Codes at or above this are framework codes shared by every contract.
const SHARED_FROM: u32 = 100;

struct Variant {
    name: String,
    code: u32,
    meaning: String,
    cause: String,
}

struct ErrorEnum {
    crate_name: String,
    path: String,
    enum_name: String,
    variants: Vec<Variant>,
}

pub fn errors(ws: &Workspace, opts: &Opts) -> Result<()> {
    let mut enums = Vec::new();
    let mut problems = Vec::new();
    for name in ws.complete() {
        let pkg = &ws.packages[name];
        if !pkg.is_contract {
            continue;
        }
        let file = pkg.dir.join("src").join("error.rs");
        if !file.exists() {
            continue;
        }
        let rel = file
            .strip_prefix(&ws.root)
            .unwrap_or(&file)
            .display()
            .to_string();
        for e in parse(&read(&file)?, name, &rel, &mut problems) {
            enums.push(e);
        }
    }
    check_shared(&enums, &mut problems);
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }

    let rendered = render(ws, &enums);
    let path = ws.root.join(REGISTRY);
    if opts.check {
        let current = fs::read_to_string(&path).unwrap_or_default();
        if current != rendered {
            return Err(format!(
                "{REGISTRY} is out of date; run `cargo xtask errors` and commit the result"
            ));
        }
        eprintln!("xtask: {REGISTRY} is up to date");
    } else {
        fs::write(&path, rendered).map_err(|e| format!("writing {REGISTRY}: {e}"))?;
        eprintln!("xtask: wrote {REGISTRY}");
    }
    Ok(())
}

fn parse(src: &str, crate_name: &str, path: &str, problems: &mut Vec<String>) -> Vec<ErrorEnum> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != "#[contracterror]" {
            i += 1;
            continue;
        }
        while i < lines.len() && !lines[i].contains("enum ") {
            i += 1;
        }
        let Some(header) = lines.get(i) else { break };
        let enum_name: String = header[header.find("enum ").map_or(0, |p| p + 5)..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        i += 1;
        let mut doc: Vec<String> = Vec::new();
        let mut variants: Vec<Variant> = Vec::new();
        while i < lines.len() && lines[i].trim() != "}" {
            let t = lines[i].trim();
            i += 1;
            if let Some(d) = t.strip_prefix("///") {
                doc.push(d.trim().to_string());
                continue;
            }
            if t.is_empty() || t.starts_with("//") || t.starts_with("#[") {
                continue;
            }
            let Some((name, code)) = t.trim_end_matches(',').split_once('=') else {
                problems.push(format!("{path}:{i}: can't parse variant `{t}`"));
                continue;
            };
            let (name, code) = (name.trim().to_string(), code.trim());
            let Ok(code) = code.parse::<u32>() else {
                problems.push(format!("{path}:{i}: `{name}` has a non-literal code"));
                continue;
            };
            let (meaning, cause) = split_doc(&doc);
            if meaning.is_empty() || cause.is_empty() {
                problems.push(format!(
                    "{path}:{i}: `{enum_name}::{name}` needs a doc comment with a meaning and a `Cause:` line"
                ));
            }
            if variants.iter().any(|v| v.code == code) {
                problems.push(format!("{path}:{i}: code {code} is used twice"));
            }
            variants.push(Variant {
                name,
                code,
                meaning,
                cause,
            });
            doc.clear();
        }
        variants.sort_by_key(|v| v.code);
        out.push(ErrorEnum {
            crate_name: crate_name.to_string(),
            path: path.to_string(),
            enum_name,
            variants,
        });
    }
    out
}

/// (meaning, cause) from a variant's doc lines.
fn split_doc(doc: &[String]) -> (String, String) {
    let mut meaning = Vec::new();
    let mut cause = Vec::new();
    let mut in_cause = false;
    for line in doc {
        if let Some(c) = line.strip_prefix("Cause:") {
            in_cause = true;
            cause.push(c.trim().to_string());
        } else if in_cause {
            cause.push(line.clone());
        } else {
            meaning.push(line.clone());
        }
    }
    let join = |v: Vec<String>| {
        v.into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    (join(meaning), join(cause))
}

fn check_shared(enums: &[ErrorEnum], problems: &mut Vec<String>) {
    let mut seen: BTreeMap<u32, (&str, &str)> = BTreeMap::new();
    for e in enums {
        for v in e.variants.iter().filter(|v| v.code >= SHARED_FROM) {
            match seen.get(&v.code) {
                Some((name, first)) if *name != v.name => problems.push(format!(
                    "{}: shared code {} is `{}` here but `{name}` in {first}",
                    e.path, v.code, v.name
                )),
                Some(_) => {}
                None => {
                    seen.insert(v.code, (&v.name, &e.path));
                }
            }
        }
    }
}

fn cell(s: &str) -> String {
    let mut chars = s.chars();
    let capitalized: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    capitalized.replace('|', "\\|")
}

/// The crate's directory (`grants_escrow`), from its error.rs path.
fn dir_name(e: &ErrorEnum) -> &str {
    e.path.split('/').next().unwrap_or(&e.crate_name)
}

fn render(ws: &Workspace, enums: &[ErrorEnum]) -> String {
    let mut md = String::new();
    md.push_str(
        "# Error registry\n\n\
         <!-- Generated by `cargo xtask errors` from each crate's src/error.rs.\n     \
         Do not edit by hand: CI runs `cargo xtask errors --check`. -->\n\n\
         Every entrypoint returns `Result<_, Error>`. A failed call surfaces as\n\
         `Error(Contract, #<code>)` in the transaction result, and as\n\
         `Err(Ok(Error::<Name>))` from a Rust `try_*` client call. Codes are\n\
         per contract: look the code up under the contract you called.\n\n\
         Codes are append-only. A variant's code is never renumbered or\n\
         reused, so a client built against an older spec still maps every\n\
         code it knows.\n\n",
    );

    // Shared framework codes, in every contract that uses the framework.
    let mut shared: BTreeMap<u32, (&Variant, Vec<&str>)> = BTreeMap::new();
    for e in enums {
        for v in e.variants.iter().filter(|v| v.code >= SHARED_FROM) {
            shared
                .entry(v.code)
                .or_insert_with(|| (v, Vec::new()))
                .1
                .push(dir_name(e));
        }
    }
    md.push_str(&format!(
        "## Shared codes ({SHARED_FROM} and up)\n\n\
         Codes from {SHARED_FROM} up come from the shared `zenith-common` frameworks\n\
         (storage migrations and upgrade safety). They mean the same thing in\n\
         every contract that defines them.\n\n\
         | Code | Name | Meaning | Typical cause | Defined in |\n\
         |---:|---|---|---|---|\n"
    ));
    for (code, (v, crates)) in &shared {
        let short: Vec<String> = crates.iter().map(|c| format!("`{c}`")).collect();
        md.push_str(&format!(
            "| {code} | `{}` | {} | {} | {} |\n",
            v.name,
            cell(&v.meaning),
            cell(&v.cause),
            short.join(", ")
        ));
    }
    md.push('\n');

    for e in enums {
        md.push_str(&format!(
            "## {}\n\n`{}` in [{}](../{}).\n\n\
             | Code | Name | Meaning | Typical cause |\n\
             |---:|---|---|---|\n",
            dir_name(e),
            e.enum_name,
            e.path,
            e.path
        ));
        for v in &e.variants {
            if v.code >= SHARED_FROM {
                continue;
            }
            md.push_str(&format!(
                "| {} | `{}` | {} | {} |\n",
                v.code,
                v.name,
                cell(&v.meaning),
                cell(&v.cause)
            ));
        }
        let shared_here: Vec<String> = e
            .variants
            .iter()
            .filter(|v| v.code >= SHARED_FROM)
            .map(|v| v.code.to_string())
            .collect();
        if !shared_here.is_empty() {
            md.push_str(&format!(
                "\nAlso defines shared codes {}.\n",
                shared_here.join(", ")
            ));
        }
        md.push('\n');
    }

    if !ws.incomplete.is_empty() {
        md.push_str(
            "## Not yet listed\n\n\
             These crates' sources are incomplete on `main` (see README \"Known\n\
             gaps\"), so their errors aren't in this registry yet:\n\n",
        );
        for name in &ws.incomplete {
            md.push_str(&format!("- `{name}`\n"));
        }
    }
    md
}
