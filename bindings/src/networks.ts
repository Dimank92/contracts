// Network config for testnet and mainnet, plus the deployed contract ids
// recorded in ../deployments.json (hand-maintained after each deployment).

import deployments from "../deployments.json" with { type: "json" };

export type NetworkName = "testnet" | "mainnet";

export interface NetworkConfig {
  networkPassphrase: string;
  rpcUrl: string;
}

export const networks: Record<NetworkName, NetworkConfig> = {
  testnet: {
    networkPassphrase: "Test SDF Network ; September 2015",
    rpcUrl: "https://soroban-testnet.stellar.org",
  },
  mainnet: {
    networkPassphrase: "Public Global Stellar Network ; September 2015",
    // Mainnet has no public SDF RPC: point this at your provider.
    rpcUrl: "",
  },
};

const ids = deployments as unknown as Record<NetworkName, Record<string, string | undefined>>;

/** The deployed id of `contract` on `network`; throws if none is recorded. */
export function contractId(network: NetworkName, contract: string): string {
  const id = ids[network]?.[contract];
  if (!id) {
    throw new Error(`no ${contract} deployment recorded for ${network} in deployments.json`);
  }
  return id;
}

/**
 * Client options for `contract` on `network`, e.g.
 * `new streams.Client(clientOptions("testnet", "streams"))`.
 */
export function clientOptions(network: NetworkName, contract: string, rpcUrl?: string) {
  const config = networks[network];
  return {
    networkPassphrase: config.networkPassphrase,
    rpcUrl: rpcUrl ?? config.rpcUrl,
    contractId: contractId(network, contract),
  };
}
