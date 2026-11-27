import {
  createBilinRuntime,
  type RuntimeWebSocket,
} from "@bilin/runtime/adapter";
import type {
  BilinEvent,
  BilinInstance,
  BilinNetworkConfig,
} from "@bilin/runtime";

export interface BrowserBilinConfig extends BilinNetworkConfig {
  ipv4: string;
  peers: string | readonly string[];
}

export interface BrowserBilinOptions {
  onEvent?: (event: BilinEvent) => void;
}

export async function createBilinWithArtifact(
  artifact: BufferSource,
  config: BrowserBilinConfig,
  options: BrowserBilinOptions = {},
): Promise<BilinInstance> {
  return createBilinRuntime({
    module: () => WebAssembly.compile(artifact),
    config: {
      profile: "browser",
      instanceId: crypto.randomUUID(),
      instanceName: config.instanceName ?? "bilin-browser",
      networkName: config.networkName,
      networkSecret: config.networkSecret,
      encryption: config.encryption ?? true,
      ipv4: config.ipv4,
      peers:
        typeof config.peers === "string" ? [config.peers] : [...config.peers],
    },
    connectWebSocket: (url) =>
      new WebSocket(url) as unknown as RuntimeWebSocket,
    onEvent: options.onEvent,
  });
}
