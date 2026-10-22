//! `xtask` — the repo's single local automation entrypoint.
//!
//! Subcommands:
//!   * `check-stubs` — walk crate `src/` trees and flag 0-byte files,
//!     `todo!()`/`unimplemented!()` macros, and the literal
//!     "implementation stub" marker. Warnings by default; `--strict` fails.
//!   * `ignores` — print file, line, and cited issue for every `#[ignore]`.
//!   * `spec` — dump a named contract's spec/ABI or scaffold its wasm build.
//!
//! Exit codes: 0 clean, 1 findings, 2 usage error. No network access.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const EXIT_CLEAN: i32 = 0;
const EXIT_FINDINGS: i32 = 1;
const EXIT_USAGE: i32 = 2;

/// Crates whose `src/` trees are intentionally generated and must be skipped.
/// Matched against the crate name (last path segment of the package name).
const DEFAULT_ALLOWLIST: &[&str] = &["tools/spec-diff"];

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("check-stubs") => cmd_check_stubs(&args[1..]),
        Some("ignores") => cmd_ignores(&args[1..]),
        Some("spec") => cmd_spec(&args[1..]),
        Some("help") | Some("--help") | Some("-h") | None => {
            print_usage();
            EXIT_CLEAN
        }
        Some(other) => {
            eprintln!("xtask: unknown subcommand `{other}`");
            print_usage();
            EXIT_USAGE
        }
    };
    process::exit(code);
}

fn print_usage() {
    eprintln!(
        "usage: xtask <command> [options]\n\n\
         commands:\n\
         \x20 check-stubs [--strict] [--allow <crate>]...\n\
         \x20 ignores\n\
         \x20 spec <crate> [--wasm]"
    );
}

// ---------------------------------------------------------------------------
// Workspace enumeration via `cargo metadata`
// ---------------------------------------------------------------------------

struct Member {
    name: String,
    manifest_dir: PathBuf,
}

/// Enumerate workspace members from `cargo metadata` (offline, no deps).
fn workspace_members() -> Result<Vec<Member>, String> {
    let output = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--no-deps", "--format-version", "1", "--offline"])
        .output()
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_metadata_members(&text))
}

/// Minimal, dependency-free extraction of `packages[].{name,manifest_path}`
/// from `cargo metadata` JSON. We only need the two fields, so a full JSON
/// parser is overkill; this scans for the `"packages"` array and pulls the
/// `name`/`manifest_path` pairs in order.
fn parse_metadata_members(json: &str) -> Vec<Member> {
    let mut members = Vec::new();
    let mut rest = json;
    while let Some(idx) = rest.find("\"manifest_path\"") {
        rest = &rest[idx + "\"manifest_path\"".len()..];
        let Some(path) = extract_json_string(rest) else { break };
        // The `name` field precedes `manifest_path` in cargo's output; look
        // backwards from the manifest_path for the nearest `"name"`.
        let before = &json[..json.len() - rest.len()];
        let name = before
            .rfind("\"name\"")
            .and_then(|i| extract_json_string(&before[i + "\"name\"".len()..]))
            .unwrap_or_default();
        let manifest = PathBuf::from(path);
        if let Some(dir) = manifest.parent() {
            members.push(Member {
                name,
                manifest_dir: dir.to_path_buf(),
            });
        }
    }
    members
}

/// Extract the first JSON string literal from `s` (which should start just
/// after a key). Returns `None` if no string is found.
fn extract_json_string(s: &str) -> Option<String> {
    let start = s.find('"')? + 1;
    let mut out = String::new();
    let mut chars = s[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

// ---------------------------------------------------------------------------
// check-stubs
// ---------------------------------------------------------------------------

fn cmd_check_stubs(args: &[String]) -> i32 {
    let mut strict = false;
    let mut allow: BTreeSet<String> = DEFAULT_ALLOWLIST.iter().map(|s| s.to_string()).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--strict" => strict = true,
            "--allow" => {
                i += 1;
                match args.get(i) {
                    Some(v) => {
                        allow.insert(v.clone());
                    }
                    None => {
                        eprintln!("xtask: --allow requires a crate name");
                        return EXIT_USAGE;
                    }
                }
            }
            other => {
                eprintln!("xtask: unknown option `{other}` for check-stubs");
                return EXIT_USAGE;
            }
        }
        i += 1;
    }

    let members = match workspace_members() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("xtask: {e}");
            return EXIT_USAGE;
        }
    };

    let mut findings: Vec<String> = Vec::new();
    for member in &members {
        let rel = rel_path(&member.manifest_dir);
        if allow.contains(&rel) || allow.contains(&member.name) {
            continue;
        }
        let src = member.manifest_dir.join("src");
        if !src.is_dir() {
            continue;
        }
        collect_stub_findings(&src, &mut findings);
    }

    findings.sort();
    findings.dedup();
    let mut out = io::stdout().lock();
    for f in &findings {
        let _ = writeln!(out, "{f}");
    }
    if findings.is_empty() {
        EXIT_CLEAN
    } else if strict {
        EXIT_FINDINGS
    } else {
        eprintln!(
            "xtask: {} stub finding(s); pass --strict to fail",
            findings.len()
        );
        EXIT_CLEAN
    }
}

fn collect_stub_findings(dir: &Path, out: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_stub_findings(&path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let rel = rel_path(&path);
        if meta.len() == 0 {
            out.push(format!("{rel}: empty file"));
            continue;
        }
        // Skip very large files; they are not stubs.
        if meta.len() > 1_000_000 {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let text = String::from_utf8_lossy(&bytes);
        for (lineno, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains("implementation stub") {
                out.push(format!("{rel}:{}: implementation stub marker", lineno + 1));
            } else if line.contains("todo!(") {
                out.push(format!("{rel}:{}: todo!()", lineno + 1));
            } else if line.contains("unimplemented!(") {
                out.push(format!("{rel}:{}: unimplemented!()", lineno + 1));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ignores
// ---------------------------------------------------------------------------

fn cmd_ignores(args: &[String]) -> i32 {
    if !args.is_empty() {
        eprintln!("xtask: `ignores` takes no arguments");
        return EXIT_USAGE;
    }
    let members = match workspace_members() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("xtask: {e}");
            return EXIT_USAGE;
        }
    };

    let mut rows: Vec<String> = Vec::new();
    for member in &members {
        let src = member.manifest_dir.join("src");
        if src.is_dir() {
            collect_ignores(&src, &mut rows);
        }
        let tests = member.manifest_dir.join("tests");
        if tests.is_dir() {
            collect_ignores(&tests, &mut rows);
        }
    }

    rows.sort();
    rows.dedup();
    let mut out = io::stdout().lock();
    for r in &rows {
        let _ = writeln!(out, "{r}");
    }
    if rows.is_empty() {
        EXIT_CLEAN
    } else {
        EXIT_FINDINGS
    }
}

fn collect_ignores(dir: &Path, out: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_ignores(&path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let text = String::from_utf8_lossy(&bytes);
        let rel = rel_path(&path);
        for (lineno, line) in text.lines().enumerate() {
            if !line.contains("#[ignore") {
                continue;
            }
            let issue = cited_issue(line).unwrap_or_else(|| "-".to_string());
            out.push(format!("{rel}:{}: {issue}", lineno + 1));
        }
    }
}

/// Find the first `#<digits>` reference in a line, e.g. "issue #115".
fn cited_issue(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                return Some(line[start..end].to_string());
            }
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// spec
// ---------------------------------------------------------------------------

fn cmd_spec(args: &[String]) -> i32 {
    let mut crate_name: Option<String> = None;
    let mut wasm = false;
    for arg in args {
        match arg.as_str() {
            "--wasm" => wasm = true,
            other if other.starts_with('-') => {
                eprintln!("xtask: unknown option `{other}` for spec");
                return EXIT_USAGE;
            }
            other => {
                if crate_name.is_some() {
                    eprintln!("xtask: spec takes a single crate name");
                    return EXIT_USAGE;
                }
                crate_name = Some(other.to_string());
            }
        }
    }
    let Some(crate_name) = crate_name else {
        eprintln!("xtask: spec requires a crate name");
        return EXIT_USAGE;
    };

    let members = match workspace_members() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("xtask: {e}");
            return EXIT_USAGE;
        }
    };
    let Some(member) = members.iter().find(|m| {
        m.name == crate_name
            || m.name.ends_with(&format!("-{crate_name}"))
            || rel_path(&m.manifest_dir) == crate_name
    }) else {
        eprintln!("xtask: no workspace member matches `{crate_name}`");
        return EXIT_USAGE;
    };

    if wasm {
        // Scaffold the wasm build for the named contract.
        let status = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["build", "--target", "wasm32-unknown-unknown", "--release", "-p"])
            .arg(&member.name)
            .status();
        return match status {
            Ok(s) if s.success() => EXIT_CLEAN,
            Ok(_) => EXIT_FINDINGS,
            Err(e) => {
                eprintln!("xtask: failed to run cargo build: {e}");
                EXIT_USAGE
            }
        };
    }

    // Dump the contract spec/ABI by running the crate's spec test if present.
    let status = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["test", "-p"])
        .arg(&member.name)
        .args(["--", "--nocapture", "spec"])
        .status();
    match status {
        Ok(s) if s.success() => EXIT_CLEAN,
        Ok(_) => EXIT_FINDINGS,
        Err(e) => {
            eprintln!("xtask: failed to run cargo test: {e}");
            EXIT_USAGE
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Render `path` relative to the current directory when possible, so output
/// is stable and diffable (no absolute paths).
fn rel_path(path: &Path) -> String {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let rel = path.strip_prefix(&cwd).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_issue_number() {
        assert_eq!(cited_issue("// issue #115: vault::deposit"), Some("115".into()));
        assert_eq!(cited_issue("#[ignore]"), None);
    }

    #[test]
    fn parses_metadata_members() {
        let json = r#"{"packages":[{"name":"zenith-vault","manifest_path":"/w/vault/Cargo.toml"}]}"#;
        let members = parse_metadata_members(json);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].name, "zenith-vault");
    }
}
