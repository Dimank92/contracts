// Runtime check of decodeEvent against the built package: `npm test`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { nativeToScVal, xdr } from "@stellar/stellar-sdk";
import { decodeEvent } from "../dist/index.js";

const sym = (s) => xdr.ScVal.scvSymbol(s);
const u32 = (n) => xdr.ScVal.scvU32(n);

test("decodes a v2 event's keys, data and state", () => {
  const topics = [sym("param_updated"), u32(2), sym("fee_bps")];
  const data = nativeToScVal(
    { new_value: 75n, old_value: 50n, updated_at: 1000n, version: 2n },
    { type: { new_value: ["symbol", "i128"], old_value: ["symbol", "i128"], updated_at: ["symbol", "u64"], version: ["symbol", "u64"] } },
  );
  const e = decodeEvent("params", topics, data.toXDR("base64"));
  assert.equal(e.name, "param_updated");
  assert.equal(e.version, 2);
  assert.deepEqual(e.keys, { key: "fee_bps" });
  assert.equal(e.data.new_value, 75n);
  assert.match(e.state, /old_value/);
});

test("decodes shared framework events a contract emits", () => {
  const data = nativeToScVal({ from: 1, to: 2 }, { type: { from: ["symbol", "u32"], to: ["symbol", "u32"] } });
  const e = decodeEvent("multisig", [sym("schema_migrated"), u32(2)], data);
  assert.deepEqual(e.data, { from: 1, to: 2 });
});

test("decodes unversioned SEP-41 events", () => {
  const e = decodeEvent("lp_token", [sym("burn"), nativeToScVal("GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN7", { type: "address" })], nativeToScVal(5n, { type: "i128" }));
  assert.equal(e.version, undefined);
  assert.equal(e.data, 5n);
});

test("returns undefined for v1 or unknown events", () => {
  assert.equal(decodeEvent("params", [sym("param_updated"), sym("fee_bps")], u32(0)), undefined);
  assert.equal(decodeEvent("params", [sym("nope"), u32(2)], u32(0)), undefined);
});
