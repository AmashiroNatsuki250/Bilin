# `@bilin/browser`

Run an outbound Bilin instance and its TCP data plane directly in a browser.
The package embeds the matching Bilin WebAssembly artifact in its JavaScript
entry point; applications do not configure a bundler loader, compile Rust, load
WASI, or work with Guest handles.

## Install

```sh
pnpm add @bilin/browser
```

## Connect to an Bilin network

```ts
import { createBilin } from "@bilin/browser";

const bilin = await createBilin({
  networkName: "office",
  networkSecret: "secret",
  ipv4: "10.144.0.10/24",
  peers: "wss://relay.example.com/",
  encryption: true,
}, {
  onEvent(event) {
    console.log(event.kind, event.message);
  },
});
```

`createBilin()` resolves after the Bilin Instance is running. When the
peer is an `@bilin/cloudflare` relay, `networkName` and `networkSecret` must
match the relay configuration. Use `ws://` for local development and `wss://`
for a deployed Worker.

## Use the TCP data plane

```ts
const status = await bilin.status();
console.log(status.state, status.connections);

const stream = await bilin.connectTcp("10.144.0.20:8080", {
  timeout: 10_000,
});
await stream.write(new TextEncoder().encode("hello"));
const response = await stream.read();
console.log(new TextDecoder().decode(response.data));
await stream.close();
await bilin.close();
```

The Browser Adapter supports `ws://` and `wss://` peers and an overlay IPv4 TCP
data plane. It does not expose native listeners, TUN, STUN, or hole punching.
The browser must support WebAssembly JSPI.

## Run the complete example

The repository contains a standalone Browser-to-Cloudflare example that only
uses the public package entries:

[`bilin-js/examples/web`](https://github.com/AmashiroNatsuki250/Bilin/tree/main/bilin-js/examples/web)

It includes the Worker configuration, local secret setup, Browser UI, health
check, deployment commands, and the expected `peer_added` result.
