# `@bilin/runtime`

Shared WebAssembly runtime used by the official Bilin Browser and
Cloudflare Adapters. Application code should normally install
`@bilin/browser` or `@bilin/cloudflare` instead.

The package root contains only cross-platform Bilin types. The
`@bilin/runtime/adapter` entry is intended for platform Adapter authors and
hides Guest memory, ABI handles, Host capability operations, and the operation
broker from application-facing Interfaces.

The runtime, Browser Adapter, and Cloudflare Adapter use the same version.
Release automation must publish `@bilin/runtime` first, followed by
`@bilin/browser` and `@bilin/cloudflare`. Platform package manifests use
an exact workspace version, which `pnpm pack` rewrites to an exact registry
dependency. Consumers never run Cargo or an npm install hook.
