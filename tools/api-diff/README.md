# api-diff

Extracts each contract's exported entrypoints (names, parameter names/types,
return types) from the spec embedded in a built wasm, as sorted JSON, and
diffs them against committed baselines in [`baselines/`](baselines).

```sh
tools/api-diff/run.sh check    # build wasm, diff against baselines (CI)
tools/api-diff/run.sh update   # regenerate baselines; review the JSON in your PR
cargo run -p api-diff -- dump path/to/contract.wasm
```

* **Fails** on a removed entrypoint, or a changed parameter name/type/order or
  return type. A rename shows as `REMOVED` + `added` (the spec says no more).
* **Passes, reports** added entrypoints.
* A wasm without a baseline prints `no baseline ... (first run)` and passes;
  bootstrap it with `update`.
* Output is deterministic: an unchanged interface regenerates a byte-identical
  file.

Complements `tools/spec-diff` (base-branch ABI guard incl. types and errors).
Out of scope: behavioural diffing, the on-chain wasm-hash allowlist.
