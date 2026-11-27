# Bilin Browser and Cloudflare example

This example connects `@bilin/browser` to an inbound
`@bilin/cloudflare` relay. Its application code imports only the two public
package entries; it does not use `@bilin/runtime`, Wasm loaders, TOML, ABI
handles, or Host Tunnel internals.

## Prepare the repository build

From the Bilin repository root:

```sh
cd bilin-js
pnpm install
pnpm --filter @bilin/web-example build:packages
```

The build starts from source and generates both required Wasm profiles. No
artifact from an earlier build is required. Published package consumers receive
the matching Wasm artifacts in the npm packages.

## Set the local secret

Copy the example file and replace its placeholder with a local secret:

```sh
cp examples/web/cloudflare/.dev.vars.example \
  examples/web/cloudflare/.dev.vars
```

The `.dev.vars` file is ignored by Git. Enter the same value in the Browser UI.

## Start the Cloudflare relay

In one terminal, from the `bilin-js` workspace:

```sh
pnpm --filter @bilin/web-example dev:cloudflare
```

Check that the Bilin Instance is running:

```sh
curl http://127.0.0.1:8787/health
```

Expected response:

```json
{"ok":true,"state":"running","connections":0}
```

## Start the Browser application

In another terminal:

```sh
pnpm --filter @bilin/web-example dev:browser
```

Open the URL printed by Vite. Keep the default relay URL
`ws://127.0.0.1:8787/`, enter the secret from `.dev.vars`, and select
**Connect**. A successful session shows `peer_added`; the relay health response
then reports one active connection.

The Browser and Worker must use the same network name and secret. Local
development uses `ws://`; a deployed Worker uses `wss://`.

## Validate the example

```sh
pnpm --filter @bilin/web-example check
```

This type-checks and bundles the Browser application, generates Cloudflare
binding types from `wrangler.jsonc`, type-checks the Worker, and runs a Wrangler
deployment dry-run.

## Deploy the relay

Authenticate Wrangler, set the secret interactively, and deploy:

```sh
pnpm --filter @bilin/web-example exec wrangler login
pnpm --filter @bilin/web-example exec wrangler secret put \
  BILIN_NETWORK_SECRET --config cloudflare/wrangler.jsonc
pnpm --filter @bilin/web-example deploy:cloudflare
```

After deployment, enter the Worker URL in the Browser UI with the `wss://`
scheme.

When the packages are published, an external application installs them with:

```sh
pnpm add @bilin/browser @bilin/cloudflare
```
