import coreBytes from "./generated/bilin_core.wasm";
import { createBilinWithArtifact } from "./create.js";
import type {
  BilinEvent,
  BilinInstance,
  BilinIpv4SocketAddress,
  BilinNetworkConfig,
  BilinOperationOptions,
  BilinState,
  BilinStatus,
  BilinTcpListener,
  BilinTcpReadResult,
  BilinTcpStream,
} from "@bilin/runtime";
import type {
  BrowserBilinConfig,
  BrowserBilinOptions,
} from "./create.js";

export type {
  BilinEvent,
  BilinInstance,
  BilinIpv4SocketAddress,
  BilinNetworkConfig,
  BilinOperationOptions,
  BilinState,
  BilinStatus,
  BilinTcpListener,
  BilinTcpReadResult,
  BilinTcpStream,
};
export type { BrowserBilinConfig, BrowserBilinOptions };

export async function createBilin(
  config: BrowserBilinConfig,
  options: BrowserBilinOptions = {},
): Promise<BilinInstance> {
  return createBilinWithArtifact(coreBytes, config, options);
}
