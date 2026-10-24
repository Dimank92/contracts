//! `cargo xtask check-determinism`: rejects floating point and
//! non-deterministic std APIs in contract crates' non-test code.
//!
//! Contract math must be fixed-point integer arithmetic so identical
//! inputs give identical Wasm results on every validator. Type-level
//! bans live in clippy (`clippy::float_arithmetic`, `disallowed_types`);
//! this source scan covers what clippy cannot see (float literals inside
//! macros, `HashMap` in contract modules).
//!
//! Rules (the one place to extend them): see `BANNED_IDENTS`,
//! `BANNED_PATHS` and the float-literal check in `scan_line`.
//! Exemptions: files named `test*.rs` / `*_test.rs`, `test_adversaries/`,
//! items under `#[cfg(test)]`, comments, and string/char literals.

use std::fs;
use std::path::{Path, PathBuf};

/// Contract crates (directory names) the rules apply to.
pub const CONTRACT_CRATES: &[&str] = &[
    "options_market",
    "price_oracle",
    "vault",
    "timelock",
    "multisig",
    "lp_pool",
    "lp_token",
    "staking",
    "streams",
    "governor",
    "grants_escrow",
    "params",
];

/// Identifiers rejected anywhere in code.
const BANNED_IDENTS: &[(&str, &str)] = &[
    ("f32", "floating-point type"),
    ("f64", "floating-point type"),
    ("HashMap", "unordered std collection (use soroban_sdk::Map)"),
    ("HashSet", "unordered std collection (use soroban_sdk::Map)"),
];

/// `::`-joined path prefixes rejected anywhere in code.
const BANNED_PATHS: &[(&str, &str)] = &[
    ("std::time", "non-deterministic std API"),
    ("std::env", "non-deterministic std API"),
];

#[derive(Debug, PartialEq)]
pub struct Violation {
    pub file: PathBuf,
    pub line: usize,
    pub construct: String,
}

/// Scans every contract crate under `root`, returning all violations.
pub fn check_workspace(root: &Path) -> Vec<Violation> {
    let mut out = Vec::new();
    for name in CONTRACT_CRATES {
        let mut files = Vec::new();
        collect(&root.join(name).join("src"), &mut files);
        files.sort();
        for file in files {
            if is_exempt_file(&file) {
                continue;
            }
            if let Ok(src) = fs::read_to_string(&file) {
                out.extend(scan_source(&file, &src));
            }
        }
    }
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn is_exempt_file(path: &Path) -> bool {
    if path
        .components()
        .any(|c| c.as_os_str() == "test_adversaries")
    {
        return true;
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    stem.starts_with("test") || stem.ends_with("_test")
}

/// Scans one source file's text (non-test code only).
pub fn scan_source(file: &Path, src: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut in_block_comment = false;
    // Brace depth at which a `#[cfg(test)]` item opened; None = not in one.
    let mut test_item: Option<i32> = None;
    let mut pending_cfg_test = false;
    let mut depth = 0i32;
    for (i, raw) in src.lines().enumerate() {
        let code = strip_line(raw, &mut in_block_comment);
        let trimmed = code.trim();
        if test_item.is_none() {
            if trimmed.starts_with("#[cfg(test)]") {
                pending_cfg_test = true;
                continue;
            }
            if pending_cfg_test && !trimmed.is_empty() && !trimmed.starts_with("#[") {
                pending_cfg_test = false;
                if !trimmed.contains('{') && trimmed.ends_with(';') {
                    continue; // `mod test;` / `use ...;`
                }
                test_item = Some(depth);
            }
        }
        for c in code.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        if let Some(open) = test_item {
            if depth <= open && code.contains('}') {
                test_item = None;
            }
            continue;
        }
        for construct in scan_line(&code) {
            out.push(Violation {
                file: file.to_path_buf(),
                line: i + 1,
                construct,
            });
        }
    }
    out
}

/// Blanks comments and string/char literal contents, keeping everything
/// else (and the line length) intact.
fn strip_line(line: &str, in_block: &mut bool) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if *in_block {
            if c == '*' && next == Some('/') {
                *in_block = false;
                i += 1;
            }
            out.push(' ');
        } else if c == '/' && next == Some('/') {
            break;
        } else if c == '/' && next == Some('*') {
            *in_block = true;
            out.push(' ');
            i += 1;
        } else if c == '"' {
            out.push('"');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            out.push('"');
        } else if c == '\'' && chars.get(i + 2) == Some(&'\'') {
            out.push_str("' '");
            i += 2;
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn scan_line(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if is_ident(chars[i]) && (i == 0 || !is_ident(chars[i - 1])) {
            let start = i;
            while i < chars.len() && is_ident(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                // Numeric token: `1.5`, `1e9`-style floats, `2f64`.
                let mut end = i;
                let prev_dot = start > 0 && chars[start - 1] == '.';
                if !prev_dot && chars.get(end) == Some(&'.') {
                    let frac = chars.get(end + 1).is_some_and(|c| c.is_ascii_digit());
                    if frac {
                        end += 1;
                        while end < chars.len() && is_ident(chars[end]) {
                            end += 1;
                        }
                        found.push(format!(
                            "float literal `{}`",
                            chars[start..end].iter().collect::<String>()
                        ));
                        i = end;
                    }
                } else if word.ends_with("f32") || word.ends_with("f64") {
                    found.push(format!("float literal `{word}`"));
                }
                continue;
            }
            if let Some((_, why)) = BANNED_IDENTS.iter().find(|(id, _)| *id == word) {
                found.push(format!("`{word}` ({why})"));
            }
            // Path prefixes: `std::time...`
            let mut path = word.clone();
            let mut j = i;
            while chars.get(j) == Some(&':') && chars.get(j + 1) == Some(&':') {
                let s = j + 2;
                let mut e = s;
                while e < chars.len() && is_ident(chars[e]) {
                    e += 1;
                }
                path.push_str("::");
                path.extend(&chars[s..e]);
                j = e;
            }
            if let Some((p, why)) = BANNED_PATHS
                .iter()
                .find(|(p, _)| path == *p || path.starts_with(&format!("{p}::")))
            {
                found.push(format!("`{p}` ({why})"));
            }
        } else {
            i += 1;
        }
    }
    found
}

pub fn run(root: &Path) -> Result<(), String> {
    let violations = check_workspace(root);
    if violations.is_empty() {
        eprintln!("xtask: no floating-point or non-deterministic APIs in contract code");
        return Ok(());
    }
    Err(violations
        .iter()
        .map(|v| format!("{}:{}: {}", v.file.display(), v.line, v.construct))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(src: &str) -> Vec<Violation> {
        scan_source(Path::new("fixture.rs"), src)
    }

    #[test]
    fn flags_f64_and_float_literals() {
        let v = scan("fn a(x: f64) -> i128 {\n    let y = 1.5;\n    0\n}\n");
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].line, v[1].line), (1, 2));
        assert!(v[0].construct.contains("f64"));
        assert!(v[1].construct.contains("1.5"));
    }

    #[test]
    fn flags_nondeterministic_apis() {
        let v = scan("use std::collections::HashMap;\nlet t = std::time::Instant::now();\n");
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn ignores_comments_strings_tuple_indexes_and_cfg_test() {
        let src = "// f64 here\n/* 1.5 */\nlet s = \"f64 1.5\";\nlet a = t.0.1;\n\
                   #[cfg(test)]\nmod tests {\n    fn f(x: f64) {}\n}\nlet ok = 10_000_000i128;\n";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn exempts_test_files_and_dirs() {
        assert!(is_exempt_file(Path::new("options_market/src/test.rs")));
        assert!(is_exempt_file(Path::new("vault/src/test_adversaries/a.rs")));
        assert!(!is_exempt_file(Path::new("options_market/src/math.rs")));
    }
}
