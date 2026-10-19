// Decodes raw contract events against events.json (schema v2).

import { scValToNative, xdr } from "@stellar/stellar-sdk";
import { eventSpec, type ContractName } from "./events.js";

export interface DecodedEvent {
  contract: ContractName;
  name: string;
  /** Schema version topic (`undefined` for standard SEP-41 events). */
  version?: number;
  /** Primary-key topics after `(name, version)`, by their spec names. */
  keys: Record<string, unknown>;
  /** The data struct (or, for SEP-41 events, the raw value). */
  data: unknown;
  /** The state change this event represents, from events.json. */
  state: string;
}

type SpecEvent = {
  name: string;
  topics: readonly { name: string; type: string }[];
  data: string;
  state: string;
  versioned?: boolean;
};

/**
 * Decodes one event that `contract` emitted, from its topics and value
 * (base64 XDR strings or `xdr.ScVal`s, as RPC `getEvents` returns them).
 * Returns `undefined` for an event events.json doesn't describe, e.g. one
 * from an older schema version.
 */
export function decodeEvent(
  contract: ContractName,
  topics: readonly (string | xdr.ScVal)[],
  value: string | xdr.ScVal,
): DecodedEvent | undefined {
  const scv = (v: string | xdr.ScVal) => (typeof v === "string" ? xdr.ScVal.fromXDR(v, "base64") : v);
  const native = topics.map((t) => scValToNative(scv(t)));
  const name = String(native[0]);
  const spec = eventSpec.contracts[contract] as unknown as {
    events: readonly SpecEvent[];
    shared_events?: readonly string[];
  };
  const shared = eventSpec.shared.events as unknown as readonly SpecEvent[];
  const event =
    spec.events.find((e) => e.name === name) ??
    (spec.shared_events?.includes(name) ? shared.find((e) => e.name === name) : undefined);
  if (!event) return undefined;
  const versioned = event.versioned !== false;
  if (versioned && native[1] !== eventSpec.schema_version) return undefined;
  const first = versioned ? 2 : 1;
  const keys: Record<string, unknown> = {};
  event.topics.forEach((t, i) => {
    keys[t.name] = native[first + i];
  });
  return {
    contract,
    name,
    version: versioned ? Number(native[1]) : undefined,
    keys,
    data: scValToNative(scv(value)),
    state: event.state,
  };
}
