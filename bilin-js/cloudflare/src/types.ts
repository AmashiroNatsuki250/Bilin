import type { DurableObject } from "cloudflare:workers";
import type { BilinNetworkConfig } from "@bilin/runtime";

export type CloudflareBilinConfig = BilinNetworkConfig;

export interface BilinDurableObjectNamespace {
  getByName(name: string): {
    fetch(request: Request): Promise<Response>;
  };
}

export interface CloudflareBilinOptions<Env> {
  namespace(env: Env): BilinDurableObjectNamespace;
  config(env: Env):
    | CloudflareBilinConfig
    | Promise<CloudflareBilinConfig>;
  objectName?: string | ((request: Request, env: Env) => string);
}

export type BilinDurableObjectClass<Env> = typeof DurableObject<Env>;

export interface BilinCloudflareApplication<Env> {
  readonly DurableObject: BilinDurableObjectClass<Env>;
  fetch(
    request: Request,
    env: Env,
    context: ExecutionContext,
  ): Promise<Response>;
}
