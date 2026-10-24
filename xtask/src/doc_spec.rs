//! `cargo xtask doc-spec-check`: diffs each contract's built-wasm spec
//! against the function tables documented for it.
//!
//! Documentation convention (deliberately simple, not general markdown):
//! * A contract's docs are the body of the `## `<dir>` reference` section
//!   of README.md, plus the whole of `docs/<dir>.md` if that file exists.
//! * Inside them, every table row whose first cell holds a code span shaped
//!   like `name(arg, arg: Type) -> Ret` documents a function; a first cell
//!   may hold several (`a(x)` / `b(x)`). Only argument *names* are compared,
//!   in order. `name(...)` documents a name and skips the argument check.
//! * A contract with docs that yield no such row is an error, never a skip.
//!
//! Known, issue-referenced gaps live in `xtask/doc-spec-allow.txt`, one
//! per line as `<dir> <missing|undocumented|drift> <fn>`, or `<dir> no-docs` /
//! `<dir> no-spec` for a crate with no tables / no readable wasm spec yet, `#` comments
//! allowed. Entries that no longer match anything are errors too, so the
//! list can only shrink.

use std::collections::{BTreeMap, BTreeSet};

pub type Sigs = BTreeMap<String, Vec<String>>;

/// `None` args = documented without an argument list to check.
pub type DocSigs = BTreeMap<String, Option<Vec<String>>>;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Drift {
    /// Documented but not implemented.
    Missing(String),
    /// Implemented but not documented.
    Undocumented(String),
    /// In both, with different argument names.
    Signature(String, Vec<String>, Vec<String>),
}

impl Drift {
    fn kind(&self) -> &'static str {
        match self {
            Drift::Missing(_) => "missing",
            Drift::Undocumented(_) => "undocumented",
            Drift::Signature(..) => "drift",
        }
    }

    fn name(&self) -> &str {
        match self {
            Drift::Missing(n) | Drift::Undocumented(n) | Drift::Signature(n, ..) => n,
        }
    }

    pub fn key(&self, krate: &str) -> String {
        format!("{krate} {} {}", self.kind(), self.name())
    }

    pub fn describe(&self) -> String {
        match self {
            Drift::Missing(n) => format!("documented but not implemented: {n}"),
            Drift::Undocumented(n) => format!("implemented but not documented: {n}"),
            Drift::Signature(n, d, s) => format!(
                "signature drift in {n}: docs ({}) vs spec ({})",
                d.join(", "),
                s.join(", ")
            ),
        }
    }
}

/// Pulls `(name, args)` out of one code span, or `None` if it isn't a
/// function signature.
fn parse_sig(span: &str) -> Option<(String, Option<Vec<String>>)> {
    let span = span.trim();
    let open = span.find('(')?;
    let name = &span[..open];
    if name.is_empty()
        || !name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    let close = span.rfind(')')?;
    if close < open {
        return None;
    }
    let inner = span[open + 1..close].trim();
    if inner == "..." || inner == "…" {
        return Some((name.to_string(), None));
    }
    let args = if inner.is_empty() {
        Vec::new()
    } else {
        // Types may nest generics/tuples with commas; split at depth 0 only.
        let (mut depth, mut start, mut out) = (0i32, 0, Vec::new());
        for (i, c) in inner.char_indices() {
            match c {
                '(' | '<' | '[' | '{' => depth += 1,
                ')' | '>' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    out.push(&inner[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        out.push(&inner[start..]);
        out.iter()
            .map(|a| a.split(':').next().unwrap_or("").trim().to_string())
            .collect()
    };
    Some((name.to_string(), Some(args)))
}

/// Function rows documented in `text` (a section body or a whole file).
pub fn parse_doc(text: &str) -> DocSigs {
    let mut out = DocSigs::new();
    for line in text.lines() {
        let line = line.trim_start();
        if !line.starts_with('|') {
            continue;
        }
        let first = line[1..].split('|').next().unwrap_or("");
        // Odd-indexed pieces between backticks are the code spans.
        for span in first.split('`').skip(1).step_by(2) {
            if let Some((name, args)) = parse_sig(span) {
                out.entry(name).or_insert(args);
            }
        }
    }
    out
}

/// The body of `## `<dir>` reference` in `readme`, up to the next `## `.
pub fn readme_section(readme: &str, dir: &str) -> Option<String> {
    let heading = format!("## `{dir}` reference");
    let mut body = String::new();
    let mut inside = false;
    for line in readme.lines() {
        if line.starts_with("## ") {
            if inside {
                break;
            }
            inside = line.trim_end() == heading;
        } else if inside {
            body.push_str(line);
            body.push('\n');
        }
    }
    inside.then_some(body).or_else(|| {
        // heading was the last line / no body: treat found-but-empty as found
        readme
            .lines()
            .any(|l| l.trim_end() == heading)
            .then(String::new)
    })
}

pub fn diff(spec: &Sigs, doc: &DocSigs) -> Vec<Drift> {
    let mut out = Vec::new();
    for (name, args) in doc {
        match spec.get(name) {
            None => out.push(Drift::Missing(name.clone())),
            Some(s) => {
                if let Some(d) = args {
                    if d != s {
                        out.push(Drift::Signature(name.clone(), d.clone(), s.clone()));
                    }
                }
            }
        }
    }
    for name in spec.keys().filter(|n| !doc.contains_key(*n)) {
        out.push(Drift::Undocumented(name.clone()));
    }
    out.sort();
    out
}

pub fn parse_allowlist(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(|l| {
            l.split('#')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|l| !l.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(items: &[(&str, &[&str])]) -> Sigs {
        items
            .iter()
            .map(|(n, a)| (n.to_string(), a.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    const DOC: &str = "\
| Function | Description |
|---|---|
| `initialize(admin, fee: u32)` | setup |
| `pause()` / `unpause()` | stop |
| `get_x(...)` | view |
";

    #[test]
    fn parses_rows_with_types_pairs_and_wildcards() {
        let d = parse_doc(DOC);
        assert_eq!(d["initialize"], Some(vec!["admin".into(), "fee".into()]));
        assert_eq!(d["pause"], Some(vec![]));
        assert_eq!(d["unpause"], Some(vec![]));
        assert_eq!(d["get_x"], None);
        assert_eq!(d.len(), 4);
    }

    #[test]
    fn nested_generic_types_do_not_split_args() {
        let d = parse_doc("| `f(a: Map<u32, u32>, b: (u32, u32))` | x |");
        assert_eq!(d["f"], Some(vec!["a".into(), "b".into()]));
    }

    #[test]
    fn clean_match() {
        let s = spec(&[
            ("initialize", &["admin", "fee"]),
            ("pause", &[]),
            ("unpause", &[]),
            ("get_x", &["k"]),
        ]);
        assert!(diff(&s, &parse_doc(DOC)).is_empty());
    }

    #[test]
    fn missing_in_spec() {
        let s = spec(&[
            ("initialize", &["admin", "fee"]),
            ("pause", &[]),
            ("get_x", &[]),
        ]);
        assert_eq!(
            diff(&s, &parse_doc(DOC)),
            vec![Drift::Missing("unpause".into())]
        );
    }

    #[test]
    fn extra_in_spec() {
        let s = spec(&[
            ("initialize", &["admin", "fee"]),
            ("pause", &[]),
            ("unpause", &[]),
            ("get_x", &[]),
            ("extra", &[]),
        ]);
        assert_eq!(
            diff(&s, &parse_doc(DOC)),
            vec![Drift::Undocumented("extra".into())]
        );
    }

    #[test]
    fn signature_mismatch() {
        let s = spec(&[
            ("initialize", &["admin"]),
            ("pause", &[]),
            ("unpause", &[]),
            ("get_x", &[]),
        ]);
        let d = diff(&s, &parse_doc(DOC));
        assert_eq!(
            d,
            vec![Drift::Signature(
                "initialize".into(),
                vec!["admin".into(), "fee".into()],
                vec!["admin".into()]
            )]
        );
        assert_eq!(d[0].key("vault"), "vault drift initialize");
    }

    #[test]
    fn readme_section_is_bounded_by_next_h2() {
        let r =
            "## `a` reference\n| `f()` | x |\n### sub\n| `g()` | y |\n## other\n| `h()` | z |\n";
        let sec = readme_section(r, "a").unwrap();
        assert!(sec.contains("`f()`") && sec.contains("`g()`") && !sec.contains("`h()`"));
        assert!(readme_section(r, "b").is_none());
    }

    #[test]
    fn allowlist_ignores_comments_and_blanks() {
        let a = parse_allowlist("# c\n\nvault missing foo  # #12\n");
        assert_eq!(a.into_iter().collect::<Vec<_>>(), vec!["vault missing foo"]);
    }
}
