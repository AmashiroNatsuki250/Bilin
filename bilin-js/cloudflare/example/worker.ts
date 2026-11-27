import { createBilinCloudflare } from "../src/index";

const bilin = createBilinCloudflare<Env>({
  namespace: (env) => env.BILIN_CORE,
  objectName: (request, env) =>
    env.BILIN_OBJECT_NAME ?? new URL(request.url).hostname,
  config: (env) => ({
    networkName: env.BILIN_NETWORK_NAME,
    networkSecret: env.BILIN_NETWORK_SECRET,
    instanceName: "cloudflare-worker",
    encryption: env.BILIN_ENABLE_ENCRYPTION !== "false",
  }),
});

export class BilinCoreObject extends bilin.DurableObject {}
export default bilin;
