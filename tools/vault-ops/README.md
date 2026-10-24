# vault-ops

Read-only operations CLI for a deployed `vault`. It answers "is the escrow
ledger honest right now?" without ad-hoc scripts. It needs the `stellar` CLI
on `PATH` and **no** signing key: every contract call runs with
`stellar contract invoke --send no` (simulation only) and events come from
`stellar events`. There is no write path; mismatches are reported, never
repaired.

```sh
cargo run -p vault-ops -- <command> --vault <C...> --source <G... public key> [--network testnet] [--json]
```

| Command | What it does |
|---|---|
| `tags <token>` | Every live (nonzero) tag for `token` with its `balance_of`. |
| `reconcile <token>` | Σ tag balances vs `get_total_escrowed(token)` vs the vault's actual `token.balance`. Exit **1** on any mismatch (shortfall, untagged funds, or index/total disagreement). |
| `audit-tag <token> <integrator> <kind>:<id> [--start-ledger N]` | Count and sum of `deposited` / `withdrawn` events for that tag, and the net. |

Exit codes: `0` ok, `1` mismatch, `2` error. A failing vault call is an
error (never a zero balance), and `get_tags` pagination must reach
`get_tag_count` with exactly that many tags or the command fails instead of
reporting a partial sum. Read all pages against one ledger snapshot; if the
index changes mid-read, rerun.

`--json` output is stable for cron monitoring, e.g.

```sh
vault-ops reconcile CTOKEN... --vault CVAULT... --source GPUBLIC... --json
# {"actual_balance":"70","ok":true,"problems":[],"recorded_sum":"70","tags":7,"total_escrowed":"70"}
```

Amounts are decimal strings (i128-safe). Example against testnet:

```sh
export V=CVAULT... T=CTOKEN... S=GPUBLIC...
vault-ops tags $T --vault $V --source $S
vault-ops reconcile $T --vault $V --source $S || echo "ledger problem"
vault-ops audit-tag $T CINTEGRATOR... series:7 --vault $V --source $S --start-ledger 1000000
```

Relies on the documented vault views (`get_tag_count`, `get_tags(cursor,
limit) -> (Vec<Tag>, next_cursor)`, `balance_of`, `get_total_escrowed`) and
the `deposited`/`withdrawn` event shapes in the README's vault reference.
Tests run against a mocked `Ledger`: `cargo test -p vault-ops`.
