//! Workspace build automation, following the cargo-xtask pattern
//! (<https://github.com/matklad/cargo-xtask>). Run as `cargo xtask <cmd>`
//! (aliased in .cargo/config.toml).
//!
//! The one thing plain `cargo` can't do for this repo is build order:
//! options_market `contractimport!`s other contracts' *compiled wasm*, so
//! that wasm has to exist in the shared `target/` before options_market (or
//! anything depending on it, like integration_tests) compiles at all, even
//! natively. Each crate declares the contracts it imports in its manifest:
//!
//! ```toml
//! [package.metadata.zenith]
//! wasm-deps = ["zenith-price-oracle", "zenith-vault"]
//! ```
//!
//! and xtask reads that via `cargo metadata`, follows path dependencies
//! (a crate linking options_market as source needs options_market's
//! wasm-deps too), and builds the wasm in topological order.
//!
//! Every contract's wasm is built with its own `cargo build -p` so no other
//! workspace member's features (in particular dev-dependency `testutils`)
//! are unified into it; `check-wasm` verifies that on the actual artifacts.

mod doc_spec;

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, String>;

const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// Strings that only show up in a contract's code/data when the host-side
/// test environment (soroban-sdk `testutils` → soroban-env-host) got linked
/// in. Custom sections (the contract spec, which carries doc comments) are
/// not scanned, only code and data.
const LEAK_MARKERS: &[&str] = &[
    "soroban-env-host",
    "soroban_env_host",
    "testutils",
    "mock_all_auths",
];

const USAGE: &str = "\
Usage: cargo xtask <command> [-p <crate>]... [-- <args>]

Commands:
  build          Build contract wasm (release, wasm32) in dependency order.
                 With -p, builds those contracts plus what they import; a
                 non-contract crate builds just the wasm it needs to compile.
  test           Build required wasm, then `cargo test` (plus each crate's
                 `extra-test` run from [package.metadata.zenith]). Args after
                 `--` go to the test binaries.
  clippy         Build required wasm, then `cargo clippy --all-targets -D warnings`.
  fmt [--check]  `cargo fmt --all`.
  check-imports  Verify every contractimport! is declared in wasm-deps and
                 points at the shared target/ directory.
  check-wasm     Inspect built wasm for linked-in testutils/host code.
  doc-spec-check Diff each contract's wasm spec against its README/docs
                 function tables (needs `build` first); see xtask/src/doc_spec.rs.
  graph          Print the wasm build order and each crate's wasm needs.
  ci             fmt --check, check-imports, build, check-wasm, doc-spec-check,
                 clippy, test.

<crate> is a package name (zenith-vault) or directory (vault). Default: all.
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "help".into());
    let rest: Vec<String> = args.collect();
    if matches!(cmd.as_str(), "help" | "-h" | "--help") {
        print!("{USAGE}");
        return Ok(());
    }
    let opts = Opts::parse(&rest)?;
    let ws = Workspace::load()?;
    match cmd.as_str() {
        "build" => build(&ws, &opts),
        "test" => test(&ws, &opts),
        "clippy" => clippy(&ws, &opts),
        "fmt" => fmt(&ws, &opts),
        "check-imports" => check_imports(&ws),
        "check-wasm" => check_wasm(&ws, &opts),
        "doc-spec-check" => doc_spec_check(&ws, &opts),
        "graph" => graph(&ws),
        "ci" => ci(&ws),
        other => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    }
}

#[derive(Default)]
struct Opts {
    packages: Vec<String>,
    check: bool,
    passthrough: Vec<String>,
}

impl Opts {
    fn parse(args: &[String]) -> Result<Opts> {
        let mut opts = Opts::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "-p" | "--package" => opts
                    .packages
                    .push(it.next().ok_or("-p needs a crate name")?.clone()),
                "--check" => opts.check = true,
                "--" => {
                    opts.passthrough = it.by_ref().cloned().collect();
                    break;
                }
                other => return Err(format!("unexpected argument `{other}`")),
            }
        }
        Ok(opts)
    }
}

/// A `cargo test` run a crate asks for on top of the default one, e.g.
/// vault's invariant-checking build plus its #[ignore]d fuzz test.
struct ExtraTest {
    features: Vec<String>,
    args: Vec<String>,
}

struct Package {
    dir: PathBuf,
    /// Library target name (`zenith_vault`), which names the wasm file.
    lib_name: Option<String>,
    /// Has a cdylib target, i.e. produces a deployable wasm.
    is_contract: bool,
    wasm_deps: Vec<String>,
    /// Workspace crates this one links as source: (package name, dev-only).
    path_deps: Vec<(String, bool)>,
    extra_test: Option<ExtraTest>,
}

struct Workspace {
    root: PathBuf,
    target_dir: PathBuf,
    packages: BTreeMap<String, Package>,
}

impl Workspace {
    fn load() -> Result<Workspace> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("xtask must live one level below the workspace root")?
            .to_path_buf();
        // contractimport! paths are relative to each crate and point at
        // <root>/target, so every cargo run below is pinned there.
        let target_dir = root.join("target");
        if let Some(dir) = env::var_os("CARGO_TARGET_DIR") {
            if Path::new(&dir) != target_dir {
                eprintln!(
                    "xtask: note: ignoring CARGO_TARGET_DIR={}; using {}",
                    Path::new(&dir).display(),
                    target_dir.display()
                );
            }
        }

        let manifest = root.join("Cargo.toml");
        let out = Command::new(cargo_bin())
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .arg("--manifest-path")
            .arg(&manifest)
            .output()
            .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "cargo metadata failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let meta: Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("bad cargo metadata output: {e}"))?;

        let mut packages = BTreeMap::new();
        for pkg in meta["packages"].as_array().ok_or("no packages")? {
            let name = str_field(pkg, "name")?;
            let dir = Path::new(str_field(pkg, "manifest_path")?)
                .parent()
                .ok_or("manifest without a directory")?
                .to_path_buf();

            let mut lib_name = None;
            let mut is_contract = false;
            for target in pkg["targets"].as_array().into_iter().flatten() {
                let kinds = strings(&target["kind"]);
                if kinds.iter().any(|k| k == "cdylib") {
                    is_contract = true;
                }
                if kinds
                    .iter()
                    .any(|k| matches!(k.as_str(), "lib" | "rlib" | "cdylib"))
                {
                    lib_name = Some(str_field(target, "name")?.replace('-', "_"));
                }
            }

            let zenith = &pkg["metadata"]["zenith"];
            let extra_test = zenith.get("extra-test").map(|t| ExtraTest {
                features: strings(&t["features"]),
                args: strings(&t["args"]),
            });

            let path_deps: Vec<(String, bool)> = pkg["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| d["path"].is_string())
                .map(|d| Ok((str_field(d, "name")?.to_string(), d["kind"] == "dev")))
                .collect::<Result<_>>()?;

            packages.insert(
                name.to_string(),
                Package {
                    dir,
                    lib_name,
                    is_contract,
                    wasm_deps: strings(&zenith["wasm-deps"]),
                    path_deps,
                    extra_test,
                },
            );
        }

        for (name, pkg) in &packages {
            for dep in &pkg.wasm_deps {
                match packages.get(dep) {
                    Some(d) if d.is_contract => {}
                    Some(_) => {
                        return Err(format!(
                            "{name}: wasm-deps entry `{dep}` is not a contract (no cdylib target)"
                        ))
                    }
                    None => {
                        return Err(format!(
                            "{name}: wasm-deps entry `{dep}` is not a workspace package"
                        ))
                    }
                }
            }
        }

        Ok(Workspace {
            root,
            target_dir,
            packages,
        })
    }

    /// Accepts a package name, a directory relative to the root, or a
    /// directory name with the `zenith-` prefix left off.
    fn resolve(&self, sel: &str) -> Result<String> {
        let sel = sel.trim_end_matches('/');
        if self.packages.contains_key(sel) {
            return Ok(sel.to_string());
        }
        let prefixed = format!("zenith-{}", sel.replace('_', "-"));
        if self.packages.contains_key(&prefixed) {
            return Ok(prefixed);
        }
        self.packages
            .iter()
            .find(|(_, p)| p.dir.strip_prefix(&self.root).ok() == Some(Path::new(sel)))
            .map(|(name, _)| name.clone())
            .ok_or_else(|| format!("no workspace crate matches `{sel}`"))
    }

    /// The requested crates, or every workspace member.
    fn selection(&self, requested: &[String]) -> Result<Vec<String>> {
        if requested.is_empty() {
            Ok(self.packages.keys().cloned().collect())
        } else {
            requested.iter().map(|r| self.resolve(r)).collect()
        }
    }

    fn contracts(&self) -> Vec<String> {
        self.packages
            .iter()
            .filter(|(_, p)| p.is_contract)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Contracts whose wasm must exist before `name` compiles: its own
    /// wasm-deps plus those of every workspace crate it links as source.
    /// Dev-dependencies count only for `name` itself (tests/clippy), not
    /// transitively — cargo never builds a dependency's dev-dependencies.
    fn wasm_needed_by(&self, name: &str, include_dev: bool) -> BTreeSet<String> {
        let mut needed = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut stack = vec![(name.to_string(), include_dev)];
        while let Some((current, dev)) = stack.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let Some(pkg) = self.packages.get(&current) else {
                continue;
            };
            needed.extend(pkg.wasm_deps.iter().cloned());
            for (dep, dev_only) in &pkg.path_deps {
                if dev || !dev_only {
                    stack.push((dep.clone(), false));
                }
            }
        }
        needed
    }

    /// Topological order in which to build `roots`' wasm, dependencies
    /// first. Errors on cycles (a contract can't import its own importer).
    fn wasm_build_order(&self, roots: &BTreeSet<String>) -> Result<Vec<String>> {
        fn visit(
            ws: &Workspace,
            name: &str,
            done: &mut BTreeSet<String>,
            path: &mut Vec<String>,
            order: &mut Vec<String>,
        ) -> Result<()> {
            if done.contains(name) {
                return Ok(());
            }
            if path.iter().any(|p| p == name) {
                return Err(format!(
                    "wasm dependency cycle: {} -> {name}",
                    path.join(" -> ")
                ));
            }
            path.push(name.to_string());
            for dep in ws.wasm_needed_by(name, false) {
                visit(ws, &dep, done, path, order)?;
            }
            path.pop();
            done.insert(name.to_string());
            order.push(name.to_string());
            Ok(())
        }

        let mut order = Vec::new();
        let mut done = BTreeSet::new();
        for root in roots {
            visit(self, root, &mut done, &mut Vec::new(), &mut order)?;
        }
        Ok(order)
    }

    fn wasm_path(&self, name: &str) -> Result<PathBuf> {
        let lib = self.packages[name]
            .lib_name
            .as_ref()
            .ok_or_else(|| format!("{name} has no library target"))?;
        Ok(self
            .target_dir
            .join(WASM_TARGET)
            .join("release")
            .join(format!("{lib}.wasm")))
    }

    fn build_wasm(&self, roots: &BTreeSet<String>) -> Result<()> {
        let order = self.wasm_build_order(roots)?;
        if order.is_empty() {
            return Ok(());
        }
        ensure_wasm_target()?;
        for (i, name) in order.iter().enumerate() {
            eprintln!("xtask: [{}/{}] wasm {name}", i + 1, order.len());
            // One invocation per contract: keeps feature unification to
            // that contract's own (non-dev) dependency graph.
            self.cargo(&[
                "build",
                "-p",
                name.as_str(),
                "--lib",
                "--target",
                WASM_TARGET,
                "--release",
            ])?;
        }
        Ok(())
    }

    /// Builds the wasm that `selection` needs before it can compile
    /// natively with all targets (tests included).
    fn build_wasm_for_native(&self, selection: &[String]) -> Result<()> {
        let roots = selection
            .iter()
            .flat_map(|name| self.wasm_needed_by(name, true))
            .collect();
        self.build_wasm(&roots)
    }

    fn package_flags(&self, requested: &[String], selection: &[String]) -> Vec<String> {
        if requested.is_empty() {
            vec!["--workspace".into()]
        } else {
            selection
                .iter()
                .flat_map(|name| ["-p".to_string(), name.clone()])
                .collect()
        }
    }

    fn cargo<S: AsRef<str>>(&self, args: &[S]) -> Result<()> {
        let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
        eprintln!("$ cargo {}", args.join(" "));
        let status = Command::new(cargo_bin())
            .args(&args)
            .current_dir(&self.root)
            .env("CARGO_TARGET_DIR", &self.target_dir)
            .status()
            .map_err(|e| format!("failed to run cargo: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("`cargo {}` failed ({status})", args.join(" ")))
        }
    }

    fn cargo_output(&self, args: &[&str]) -> Result<String> {
        let out = Command::new(cargo_bin())
            .args(args)
            .current_dir(&self.root)
            .env("CARGO_TARGET_DIR", &self.target_dir)
            .output()
            .map_err(|e| format!("failed to run cargo: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "`cargo {}` failed:\n{}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

fn cargo_bin() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| format!("cargo metadata: missing `{key}`"))
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(String::from))
        .collect()
}

fn ensure_wasm_target() -> Result<()> {
    // Only a friendlier error than rustc's; skipped without rustup.
    let Ok(out) = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
    else {
        return Ok(());
    };
    if out.status.success()
        && !String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.trim() == WASM_TARGET)
    {
        return Err(format!(
            "the {WASM_TARGET} target isn't installed; run `rustup target add {WASM_TARGET}`"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- commands

fn build(ws: &Workspace, opts: &Opts) -> Result<()> {
    if opts.packages.is_empty() {
        return ws.build_wasm(&ws.contracts().into_iter().collect());
    }
    let mut roots = BTreeSet::new();
    for name in ws.selection(&opts.packages)? {
        if ws.packages[&name].is_contract {
            roots.insert(name);
        } else {
            roots.extend(ws.wasm_needed_by(&name, true));
        }
    }
    ws.build_wasm(&roots)
}

fn test(ws: &Workspace, opts: &Opts) -> Result<()> {
    let selection = ws.selection(&opts.packages)?;
    ws.build_wasm_for_native(&selection)?;

    let mut args = vec!["test".to_string()];
    args.extend(ws.package_flags(&opts.packages, &selection));
    if !opts.passthrough.is_empty() {
        args.push("--".into());
        args.extend(opts.passthrough.iter().cloned());
    }
    ws.cargo(&args)?;

    for name in &selection {
        let Some(extra) = &ws.packages[name].extra_test else {
            continue;
        };
        let mut args = vec!["test".to_string(), "-p".into(), name.clone()];
        if !extra.features.is_empty() {
            args.push("--features".into());
            args.push(extra.features.join(","));
        }
        if !extra.args.is_empty() {
            args.push("--".into());
            args.extend(extra.args.iter().cloned());
        }
        ws.cargo(&args)?;
    }
    Ok(())
}

fn clippy(ws: &Workspace, opts: &Opts) -> Result<()> {
    let selection = ws.selection(&opts.packages)?;
    ws.build_wasm_for_native(&selection)?;
    let mut args = vec!["clippy".to_string()];
    args.extend(ws.package_flags(&opts.packages, &selection));
    args.extend(["--all-targets", "--", "-D", "warnings"].map(String::from));
    ws.cargo(&args)
}

fn fmt(ws: &Workspace, opts: &Opts) -> Result<()> {
    if opts.check {
        ws.cargo(&["fmt", "--all", "--", "--check"])
    } else {
        ws.cargo(&["fmt", "--all"])
    }
}

fn graph(ws: &Workspace) -> Result<()> {
    let order = ws.wasm_build_order(&ws.contracts().into_iter().collect())?;
    println!("wasm build order:");
    for (i, name) in order.iter().enumerate() {
        let deps = ws.wasm_needed_by(name, false);
        if deps.is_empty() {
            println!("  {:>2}. {name}", i + 1);
        } else {
            let deps: Vec<_> = deps.into_iter().collect();
            println!("  {:>2}. {name}  (imports {})", i + 1, deps.join(", "));
        }
    }
    println!("\nwasm needed before native build/test:");
    for name in ws.packages.keys() {
        let deps = ws.wasm_needed_by(name, true);
        if !deps.is_empty() {
            let deps: Vec<_> = deps.into_iter().collect();
            println!("  {name}: {}", deps.join(", "));
        }
    }
    Ok(())
}

fn check_imports(ws: &Workspace) -> Result<()> {
    let mut errors = Vec::new();
    for (name, pkg) in &ws.packages {
        let mut files = Vec::new();
        collect_rs_files(&pkg.dir.join("src"), &mut files)?;
        let mut imported = BTreeSet::new();
        for file in files {
            let src = fs::read_to_string(&file)
                .map_err(|e| format!("reading {}: {e}", file.display()))?;
            for path in contractimport_paths(&src) {
                let resolved = normalize(&pkg.dir.join(&path));
                let stem = resolved
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default();
                let Some((dep, _)) = ws
                    .packages
                    .iter()
                    .find(|(_, p)| p.is_contract && p.lib_name.as_deref() == Some(stem))
                else {
                    errors.push(format!(
                        "{}: contractimport! of `{path}` matches no workspace contract",
                        file.display()
                    ));
                    continue;
                };
                if resolved != normalize(&ws.wasm_path(dep)?) {
                    errors.push(format!(
                        "{}: contractimport! path `{path}` must point at the shared \
                         target/{WASM_TARGET}/release/{stem}.wasm",
                        file.display()
                    ));
                }
                if !pkg.wasm_deps.contains(dep) {
                    errors.push(format!(
                        "{}: imports {dep}'s wasm but {name} doesn't list it in \
                         [package.metadata.zenith] wasm-deps",
                        file.display()
                    ));
                }
                imported.insert(dep.clone());
            }
        }
        for dep in &pkg.wasm_deps {
            if !imported.contains(dep) {
                errors.push(format!(
                    "{name}: wasm-deps lists {dep} but no contractimport! uses it"
                ));
            }
        }
    }
    if errors.is_empty() {
        eprintln!("xtask: contractimport! paths and wasm-deps agree");
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect_rs_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// The `file = "..."` argument of every `contractimport!` outside comments.
fn contractimport_paths(src: &str) -> BTreeSet<String> {
    let code: String = src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut paths = BTreeSet::new();
    let mut rest = code.as_str();
    while let Some(i) = rest.find("contractimport!") {
        rest = &rest[i + "contractimport!".len()..];
        let Some(end) = rest.find(')') else { break };
        let args = &rest[..end];
        if let Some(f) = args.find("file") {
            let after = &args[f..];
            let mut quoted = after.split('"');
            if let (Some(_), Some(path)) = (quoted.next(), quoted.next()) {
                paths.insert(path.to_string());
            }
        }
        rest = &rest[end..];
    }
    paths
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn check_wasm(ws: &Workspace, opts: &Opts) -> Result<()> {
    let contracts: Vec<String> = if opts.packages.is_empty() {
        ws.contracts()
    } else {
        ws.selection(&opts.packages)?
            .into_iter()
            .filter(|n| ws.packages[n].is_contract)
            .collect()
    };
    let mut errors = Vec::new();
    for name in &contracts {
        // 1. The resolved feature graph for the wasm target must not turn
        //    on soroban-sdk/testutils or pull in the host.
        let tree = ws.cargo_output(&[
            "tree",
            "-p",
            name.as_str(),
            "--target",
            WASM_TARGET,
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p} [{f}]",
        ])?;
        for line in tree.lines() {
            if line.starts_with("soroban-env-host ")
                || (line.starts_with("soroban-sdk ") && line.contains("testutils"))
            {
                errors.push(format!("{name}: wasm dependency graph has `{line}`"));
            }
        }

        // 2. The artifact itself.
        let path = ws.wasm_path(name)?;
        let bytes = fs::read(&path).map_err(|e| {
            format!(
                "{name}: can't read {} ({e}); run `cargo xtask build` first",
                path.display()
            )
        })?;
        let module = parse_wasm(&bytes).map_err(|e| format!("{name}: {e}"))?;
        if !module.custom_sections.iter().any(|s| s == "contractspecv0") {
            errors.push(format!("{name}: no contractspecv0 section"));
        }
        for export in &module.exports {
            if LEAK_MARKERS.iter().any(|m| export.contains(m)) {
                errors.push(format!("{name}: exports `{export}`"));
            }
        }
        for marker in LEAK_MARKERS {
            if module
                .code_and_data
                .iter()
                .any(|body| contains(body, marker.as_bytes()))
            {
                errors.push(format!("{name}: code/data contains `{marker}`"));
            }
        }
        eprintln!(
            "xtask: {name}: {} bytes, {} exports",
            bytes.len(),
            module.exports.len()
        );
    }
    if errors.is_empty() {
        eprintln!(
            "xtask: no testutils/host code in {} contracts",
            contracts.len()
        );
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// Function name -> argument names, from the contract spec in built wasm.
fn wasm_spec(path: &Path) -> Result<doc_spec::Sigs> {
    use stellar_xdr::curr::ScSpecEntry;
    let bytes = fs::read(path).map_err(|e| {
        format!(
            "can't read {} ({e}); run `cargo xtask build` first",
            path.display()
        )
    })?;
    let entries = soroban_spec::read::from_wasm(&bytes)
        .map_err(|e| format!("{}: can't read contract spec: {e:?}", path.display()))?;
    Ok(entries
        .into_iter()
        .filter_map(|e| match e {
            ScSpecEntry::FunctionV0(f) => Some((
                f.name.0.to_utf8_string_lossy(),
                f.inputs
                    .iter()
                    .map(|i| i.name.to_utf8_string_lossy())
                    .collect(),
            )),
            _ => None,
        })
        .collect())
}

fn doc_spec_check(ws: &Workspace, opts: &Opts) -> Result<()> {
    let contracts: Vec<String> = if opts.packages.is_empty() {
        ws.contracts()
    } else {
        ws.selection(&opts.packages)?
            .into_iter()
            .filter(|n| ws.packages[n].is_contract)
            .collect()
    };
    let readme = fs::read_to_string(ws.root.join("README.md"))
        .map_err(|e| format!("can't read README.md: {e}"))?;
    let allow_path = ws.root.join("xtask/doc-spec-allow.txt");
    let allow = doc_spec::parse_allowlist(&fs::read_to_string(&allow_path).unwrap_or_default());
    let (mut errors, mut used) = (Vec::new(), BTreeSet::new());
    for name in &contracts {
        let dir = ws.packages[name]
            .dir
            .file_name()
            .and_then(|d| d.to_str())
            .ok_or("package directory without a name")?
            .to_string();
        let mut text = doc_spec::readme_section(&readme, &dir).unwrap_or_default();
        let file = ws.root.join("docs").join(format!("{dir}.md"));
        if file.exists() {
            text.push_str(
                &fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?,
            );
        }
        let doc = doc_spec::parse_doc(&text);
        if doc.is_empty() {
            let key = format!("{dir} no-docs");
            if allow.contains(&key) {
                used.insert(key);
            } else {
                errors.push(format!(
                    "{dir}: no function tables found in its README section or docs/{dir}.md"
                ));
            }
            continue;
        }
        let spec = match wasm_spec(&ws.wasm_path(name)?) {
            Ok(spec) => spec,
            Err(e) => {
                let key = format!("{dir} no-spec");
                if allow.contains(&key) {
                    used.insert(key);
                } else {
                    errors.push(format!("{dir}: {e}"));
                }
                continue;
            }
        };
        for d in doc_spec::diff(&spec, &doc) {
            let key = d.key(&dir);
            if allow.contains(&key) {
                used.insert(key);
            } else {
                errors.push(format!("{dir}: {}", d.describe()));
            }
        }
    }
    // An entry for a crate outside this run's selection can't be judged.
    let scoped: BTreeSet<String> = contracts
        .iter()
        .filter_map(|n| ws.packages[n].dir.file_name()?.to_str().map(String::from))
        .collect();
    for stale in allow.difference(&used) {
        if scoped.contains(stale.split(' ').next().unwrap_or("")) {
            errors.push(format!("stale allowlist entry (no longer drifts): {stale}"));
        }
    }
    if errors.is_empty() {
        eprintln!(
            "xtask: doc-spec-check: {} contracts match their docs",
            contracts.len()
        );
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

#[derive(Default)]
struct WasmModule<'a> {
    exports: Vec<String>,
    custom_sections: Vec<String>,
    code_and_data: Vec<&'a [u8]>,
}

/// Just enough of the wasm binary format to list exports and custom
/// sections and find the code (10) and data (11) sections.
fn parse_wasm(bytes: &[u8]) -> Result<WasmModule<'_>> {
    if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
        return Err("not a wasm module".into());
    }
    let mut module = WasmModule::default();
    let mut pos = 8;
    while pos < bytes.len() {
        let id = bytes[pos];
        pos += 1;
        let size = leb_u32(bytes, &mut pos)? as usize;
        let end = pos
            .checked_add(size)
            .filter(|&e| e <= bytes.len())
            .ok_or("truncated section")?;
        let body = &bytes[pos..end];
        let mut p = 0;
        match id {
            0 => module.custom_sections.push(wasm_name(body, &mut p)?),
            7 => {
                for _ in 0..leb_u32(body, &mut p)? {
                    module.exports.push(wasm_name(body, &mut p)?);
                    p += 1; // export kind
                    leb_u32(body, &mut p)?; // index
                }
            }
            10 | 11 => module.code_and_data.push(body),
            _ => {}
        }
        pos = end;
    }
    Ok(module)
}

fn leb_u32(bytes: &[u8], pos: &mut usize) -> Result<u32> {
    let mut result = 0u32;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(*pos).ok_or("truncated LEB128")?;
        *pos += 1;
        result |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 35 {
            return Err("LEB128 too long".into());
        }
    }
}

fn wasm_name(bytes: &[u8], pos: &mut usize) -> Result<String> {
    let len = leb_u32(bytes, pos)? as usize;
    let s = bytes.get(*pos..*pos + len).ok_or("truncated name")?;
    *pos += len;
    Ok(String::from_utf8_lossy(s).into_owned())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn ci(ws: &Workspace) -> Result<()> {
    let all = Opts::default();
    let check = Opts {
        check: true,
        ..Opts::default()
    };
    let steps: [(&str, &dyn Fn() -> Result<()>); 7] = [
        ("fmt --check", &|| fmt(ws, &check)),
        ("check-imports", &|| check_imports(ws)),
        ("build", &|| build(ws, &all)),
        ("check-wasm", &|| check_wasm(ws, &all)),
        ("doc-spec-check", &|| doc_spec_check(ws, &all)),
        ("clippy", &|| clippy(ws, &all)),
        ("test", &|| test(ws, &all)),
    ];
    let started = Instant::now();
    let mut timings: Vec<(&str, Duration)> = Vec::new();
    for (name, step) in steps {
        eprintln!("\nxtask ci: == {name}");
        let t = Instant::now();
        let result = step();
        timings.push((name, t.elapsed()));
        if let Err(e) = result {
            print_timings(&timings, started.elapsed());
            return Err(format!("ci step `{name}` failed: {e}"));
        }
    }
    print_timings(&timings, started.elapsed());
    Ok(())
}

fn print_timings(timings: &[(&str, Duration)], total: Duration) {
    eprintln!("\nxtask ci: timings");
    for (name, d) in timings {
        eprintln!("  {name:<14} {:>7.1}s", d.as_secs_f64());
    }
    eprintln!("  {:<14} {:>7.1}s", "total", total.as_secs_f64());
}
