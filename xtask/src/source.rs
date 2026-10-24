//! Just enough Rust source handling for the lint and registry commands:
//! which files a crate actually compiles, with `#[cfg(test)]` items and
//! comments blanked out.

use std::fs;
use std::path::{Path, PathBuf};

use crate::Result;

/// Every non-test module file reachable from `src/lib.rs` through `mod`
/// declarations. Files nothing declares (left behind by a refactor) and
/// `#[cfg(test)] mod` targets are excluded.
pub fn compiled_files(crate_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let lib = crate_dir.join("src").join("lib.rs");
    if lib.exists() {
        visit(&lib, true, &mut out)?;
    }
    Ok(out)
}

fn visit(file: &Path, is_root: bool, out: &mut Vec<PathBuf>) -> Result<()> {
    if out.iter().any(|f| f == file) {
        return Ok(());
    }
    out.push(file.to_path_buf());
    let src = read(file)?;
    let code = strip(&src);
    // `foo.rs` declares its children in `foo/`; lib.rs and mod.rs in their
    // own directory.
    let dir = if is_root || file.file_name().is_some_and(|n| n == "mod.rs") {
        file.parent().map(Path::to_path_buf)
    } else {
        file.parent()
            .zip(file.file_stem())
            .map(|(p, stem)| p.join(stem))
    }
    .ok_or_else(|| format!("{}: no parent directory", file.display()))?;
    for name in mod_decls(&code) {
        let flat = dir.join(format!("{name}.rs"));
        let nested = dir.join(&name).join("mod.rs");
        if flat.exists() {
            visit(&flat, false, out)?;
        } else if nested.exists() {
            visit(&nested, false, out)?;
        }
    }
    Ok(())
}

pub fn read(file: &Path) -> Result<String> {
    fs::read_to_string(file).map_err(|e| format!("reading {}: {e}", file.display()))
}

/// Names of `mod x;` declarations (file modules) in already-stripped code.
fn mod_decls(code: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in code.lines() {
        let line = line.trim();
        let rest = line
            .strip_prefix("pub mod ")
            .or_else(|| line.strip_prefix("pub(crate) mod "))
            .or_else(|| line.strip_prefix("mod "));
        if let Some(name) = rest.and_then(|r| r.strip_suffix(';')) {
            names.push(name.trim().to_string());
        }
    }
    names
}

/// `src` with comments and every test-only (`#[cfg(test)]`,
/// `#[cfg(all(test, ..))]`) item replaced by blank lines, so line numbers
/// still match the original file.
pub fn strip(src: &str) -> String {
    let lines: Vec<String> = src.lines().map(strip_line_comment).collect();
    let mut keep = vec![true; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if is_test_cfg(lines[i].trim()) {
            let end = item_end(&lines, i + 1);
            for k in keep.iter_mut().take(end + 1).skip(i) {
                *k = false;
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    lines
        .iter()
        .zip(keep)
        .map(|(l, k)| if k { l.as_str() } else { "" })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `#[cfg(test)]`, `#[cfg(all(test, ...))]` and the like: an item only
/// compiled into tests.
fn is_test_cfg(attr: &str) -> bool {
    attr.strip_prefix("#[cfg(")
        .and_then(|r| r.strip_suffix(")]"))
        .is_some_and(|inner| {
            inner == "test" || (inner.starts_with("all(") && inner.contains("test"))
        })
}

/// Index of the last line of the item starting at or after `start`
/// (skipping further attributes): its `;` line, or its closing brace.
fn item_end(lines: &[String], start: usize) -> usize {
    let mut depth = 0i32;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        let t = line.trim();
        if !opened && (t.is_empty() || t.starts_with("#[")) {
            continue;
        }
        for c in line.chars() {
            match c {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if (opened && depth <= 0) || (!opened && t.ends_with(';')) {
            return i;
        }
    }
    lines.len().saturating_sub(1)
}

/// Drops a trailing `// ...` comment, ignoring `//` inside string literals.
fn strip_line_comment(line: &str) -> String {
    let mut in_str = false;
    let mut prev = '\0';
    let bytes: Vec<char> = line.chars().collect();
    for (i, &c) in bytes.iter().enumerate() {
        if c == '"' && prev != '\\' {
            in_str = !in_str;
        }
        if !in_str && c == '/' && bytes.get(i + 1) == Some(&'/') {
            return bytes[..i].iter().collect();
        }
        prev = c;
    }
    line.to_string()
}

/// Byte ranges of the bodies of every inherent `#[contractimpl] impl X`
/// block (trait impls such as `impl token::Interface for X` are skipped:
/// their signatures are fixed by the trait).
pub fn contractimpl_bodies(code: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = code[from..].find("#[contractimpl]") {
        let attr = from + at;
        let Some(impl_at) = code[attr..].find("impl") else {
            break;
        };
        let impl_at = attr + impl_at;
        let Some(open) = code[impl_at..].find('{') else {
            break;
        };
        let open = impl_at + open;
        let header = &code[impl_at..open];
        let close = matching_brace(code, open);
        if !header.contains(" for ") {
            out.push((open, close));
        }
        from = close.max(attr + 1);
    }
    out
}

/// Index of the `}` matching the `{` at `open`.
pub fn matching_brace(code: &str, open: usize) -> usize {
    let mut depth = 0i32;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return open + i;
                }
            }
            _ => {}
        }
    }
    code.len()
}

pub fn line_of(code: &str, offset: usize) -> usize {
    code[..offset].matches('\n').count() + 1
}
