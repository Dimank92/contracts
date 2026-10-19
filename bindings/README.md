# @zenith/contracts

Typed TypeScript clients for the Zenith Soroban contracts, generated from
their on-chain contract spec, plus network config and an event decoder
driven by [events.json](../events.json).

```ts
import { streams, clientOptions, decodeEvent } from "@zenith/contracts";

const client = new streams.Client(clientOptions("testnet", "streams"));
const { result } = await client.get_stream({ stream_id: 0n });
```

- **One namespace per contract** (`governor`, `grants_escrow`, `lp_pool`,
  `lp_token`, `multisig`, `params`, `staking`, `streams`). Each exports
  `Client`, its types and its `Errors` (code to name, matching
  [docs/errors.md](../docs/errors.md)). Namespacing keeps same-named types
  from different contracts (`InitializedEvent`, `DataKey`) apart.
- **Every method returns `Result`.** Contract errors come back as typed
  `Err` values instead of opaque host errors.
- **Network config:** `networks.testnet` / `networks.mainnet` hold the
  passphrase and RPC URL. Deployed ids live in
  [deployments.json](deployments.json), read via `contractId(network,
  contract)` and `clientOptions(network, contract)`.
- **Events:** `decodeEvent(contract, topics, value)` decodes an RPC event
  against the schema-v2 spec and returns its keys, data and the state
  change it represents. `eventSpec` is events.json itself.

Example: [examples/read-stream.ts](examples/read-stream.ts).

## Regenerating

`src/contracts/*.ts`, `src/index.ts` and `src/events.ts` are generated.
Don't edit them by hand:

```sh
cargo xtask bindings          # needs stellar-cli on PATH
cargo xtask bindings --check  # what CI runs: fails if they're stale
```

CI also runs `npm run typecheck` (`tsc --noEmit`, examples included) and
`npm test`. Pushing a `v*` tag publishes the package to npm
(`.github/workflows/release.yml`).
