import { createBilinCloudflare } from "@bilin/cloudflare";

const bilin = createBilinCloudflare<Env>({
  namespace: (env) => env.BILIN_CORE,
  config: (env) => ({
    networkName: env.BILIN_NETWORK_NAME,
    networkSecret: env.BILIN_NETWORK_SECRET,
    instanceName: "bilin-web-example-relay",
    encryption: true,
  }),
});

export class BilinCoreObject extends bilin.DurableObject {}
export default bilin;
