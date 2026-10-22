# Crate layering rules

Contracts talk to other contracts **only** through `env.invoke_contract` /
`contractimport!` against the compiled wasm, never by linking a sibling
contract crate as a Rust library. Linking one bakes its code into the
caller's wasm, couples upgrade schedules and inflates contract size.

`common/` (`zenith-common`) is the only sanctioned shared library. No other
crate may be a library dependency of more than one contract crate.

## Allowed graph

Declared in [`xtask/layering.json`](../xtask/layering.json):

| Crate | May depend on (normal deps) |
| --- | --- |
| `zenith-governor`, `zenith-grants-escrow`, `zenith-options-market`, `zenith-price-oracle`, `zenith-vault` | `zenith-common` |
| every other workspace crate | nothing |

Dev-dependencies (e.g. a sibling registered in a test `Env`) are not checked.
Target-specific and build dependencies are treated as normal ones.

## Running the check

```
cargo xtask check-layering
```

It also runs in `cargo xtask ci` and in the CI `clippy` job. Every contract
crate must have an entry in `allowed`, and every entry must be a workspace
member, so adding or removing a crate without updating the file fails.

## Adding a legitimate edge

1. Prefer `contractimport!` (declare it in `wasm-deps`) over a Rust dependency.
2. If a library edge is truly needed, add the dependency to the crate's
   `allowed` list in `xtask/layering.json` in the same PR and justify it in the
   description. A non-`common` library used by two contracts is rejected
   regardless; move the shared code into `common/` instead.
