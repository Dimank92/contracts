// Reads a stream and its balances through the generated client.
//
//   STREAMS_ID=C... STREAM_ID=0 npx tsx examples/read-stream.ts
//
// The issue (#125) asks for a "read a series" example; options_market's
// sources are incomplete on main (README "Known gaps"), so this reads the
// equivalent object of a contract that has bindings today. Swap in
// `options_market.Client` and `get_series` once that contract is back.

import { networks, streams } from "../src/index.js";

async function main() {
  const contractId = process.env.STREAMS_ID;
  if (!contractId) throw new Error("set STREAMS_ID to a deployed streams contract");
  const streamId = BigInt(process.env.STREAM_ID ?? "0");

  const client = new streams.Client({ ...networks.testnet, contractId });

  // Views are simulated, not submitted: `.result` is the decoded value.
  const { result: stream } = await client.get_stream({ stream_id: streamId });
  const found = stream.unwrap();
  if (!found) {
    console.log(`stream ${streamId} doesn't exist`);
    return;
  }
  const [withdrawable, refundable] = (await client.balance_of({ stream_id: streamId })).result.unwrap();
  console.log({
    stream: streamId.toString(),
    recipient: found.recipient,
    total: found.total.toString(),
    withdrawn: found.withdrawn.toString(),
    withdrawable: withdrawable.toString(),
    refundable: refundable.toString(),
  });
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
