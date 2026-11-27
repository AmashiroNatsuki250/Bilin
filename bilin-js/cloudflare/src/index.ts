import coreModule from "./generated/bilin_core.wasm";

import { createCloudflareApplication } from "./app.js";
import type {
  CloudflareBilinOptions,
  BilinCloudflareApplication,
} from "./types.js";

export type {
  CloudflareBilinConfig,
  CloudflareBilinOptions,
  BilinCloudflareApplication,
} from "./types.js";

export function createBilinCloudflare<Env>(
  options: CloudflareBilinOptions<Env>,
): BilinCloudflareApplication<Env> {
  return createCloudflareApplication(coreModule, options);
}
