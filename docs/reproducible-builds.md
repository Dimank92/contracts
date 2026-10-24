# Reproducible wasm builds

Two clean builds of the same commit produce byte-identical wasm, so a
published sha256 can be verified by anyone.

## Reproduce a published hash

```sh
git clone <repo> && cd contracts && git checkout <commit>
rustup show                      # installs the toolchain in rust-toolchain.toml
scripts/reproducible-build/verify.sh --print
```

Compare the printed `sha256  zenith_<crate>.wasm` lines with the published
ones. Run it without `--print` to build twice and assert both builds match.

## What makes it deterministic

- `rust-toolchain.toml` pins rustc; `Cargo.lock` is committed and CI fails
  if a build changes it.
- `.cargo/config.toml` disables incremental compilation and debuginfo.
- `verify.sh` remaps the checkout, `CARGO_HOME` and sysroot paths, and
  builds from a clean `target/wasm32-unknown-unknown`.
- Hashes cover the raw wasm from `cargo xtask build` (before any
  `wasm-opt`, which the size-budget job runs separately).

## Negative check

`VERIFY_INJECT_DRIFT=1 scripts/reproducible-build/verify.sh` injects a
time-dependent rustc flag and must exit non-zero; CI asserts this.
