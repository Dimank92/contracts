//! `cargo xtask check-panics`: every contract entrypoint fails with a
//! typed `Error`, never an untyped host trap (#124).
//!
//! 1. No `.unwrap()`, `.expect(..)`, `panic!`, `unreachable!`, `todo!` or
//!    `unimplemented!` in any non-test file a contract (or zenith-common)
//!    compiles. Use `.ok_or(Error::X)?` instead.
//! 2. Every `pub fn` in an inherent `#[contractimpl]` block returns
//!    `Result<_, _>`, so `try_*` clients always get a typed error.

use crate::source::{compiled_files, contractimpl_bodies, line_of, matching_brace, read, strip};
use crate::{Result, Workspace};

const FORBIDDEN: &[&str] = &[
    ".unwrap()",
    ".expect(",
    "panic!(",
    "unreachable!(",
    "todo!(",
    "unimplemented!(",
];

pub fn check_panics(ws: &Workspace) -> Result<()> {
    let mut errors = Vec::new();
    let mut files_checked = 0;
    let mut fns_checked = 0;
    for name in ws.complete() {
        let pkg = &ws.packages[name];
        if !(pkg.is_contract || name == "zenith-common") {
            continue;
        }
        for file in compiled_files(&pkg.dir)? {
            let rel = file.strip_prefix(&ws.root).unwrap_or(&file).display();
            let code = strip(&read(&file)?);
            files_checked += 1;
            for (n, line) in code.lines().enumerate() {
                for pat in FORBIDDEN {
                    // `panic_with_error!(` (a typed error) doesn't
                    // contain `panic!(`, so it isn't flagged.
                    if line.contains(pat) {
                        errors.push(format!("{rel}:{}: `{pat}` in contract code", n + 1));
                    }
                }
            }
            for (open, close) in contractimpl_bodies(&code) {
                let body = &code[open..close];
                let mut from = 0;
                while let Some(at) = body[from..].find("pub fn ") {
                    let start = from + at;
                    let sig_end = body[start..]
                        .find('{')
                        .map(|i| start + i)
                        .unwrap_or(body.len());
                    let sig = &body[start..sig_end];
                    fns_checked += 1;
                    if !sig.contains("-> Result<") {
                        let fn_name: String = sig["pub fn ".len()..]
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        errors.push(format!(
                            "{rel}:{}: entrypoint `{fn_name}` must return Result<_, Error>",
                            line_of(&code, open + start)
                        ));
                    }
                    from = if sig_end < body.len() {
                        matching_brace(body, sig_end) + 1
                    } else {
                        body.len()
                    };
                }
            }
        }
    }
    if errors.is_empty() {
        eprintln!(
            "xtask: no untyped panics in {files_checked} files; {fns_checked} entrypoints return Result"
        );
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}
