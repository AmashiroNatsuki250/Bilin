# CI

[中文](CI.zh-CN.md)

## Installer releases

Run **Bilin Installers** manually in GitHub Actions to validate the complete installer matrix. The default manual run stores packages in its Artifacts. A `vX.Y.Z` tag matching `bilin-gui/src-tauri/tauri.conf.json` builds the same packages and publishes them in Releases with `SHA256SUMS`.

- Windows provides NSIS installers for x64, x86, and ARM64.
- macOS provides separate Intel and Apple Silicon DMGs with ad-hoc signing. Users may need to allow the app in System Settings.
- Linux provides DEBs for x64 and ARM64.
- Android provides APKs for ARM64, ARMv7, x86, and x86_64. The default preview uses a separate app ID and a runner-generated key. It can be installed alongside the production app and requires a fresh installation when the preview signing key changes.

Set the repository Actions variable `BILIN_DEFAULT_CONTROLLER_URL` to preconfigure distributed clients. `BILIN_STUN_SERVERS` and `BILIN_STUN_UDP_FALLBACK` are optional Actions variables for NAT traversal.

To build Android release APKs that can update existing installations, configure the same production signing key with `BILIN_ANDROID_KEYSTORE_BASE64`, `BILIN_ANDROID_KEY_ALIAS`, `BILIN_ANDROID_KEY_PASSWORD`, and `BILIN_ANDROID_STORE_PASSWORD` in repository Actions Secrets.

The iOS integration requires an application Xcode project and Apple provisioning before an installable iOS package can be produced.

## Existing build workflows

| Trigger | Core | GUI | Mobile |
| --- | --- | --- | --- |
| PR targeting `main`/`develop` | Core/Web `cargo check` on Linux x64, Windows x64, and macOS ARM64 | Linux frontend build and Rust check | Android ARM64 frontend build and Rust check |
| Push to `releases/**`, or manual dispatch | 16 targets and Magisk artifacts | Installers for 7 targets | APKs for 4 architectures |

Test runs CI script tests, fmt, clippy, WASI, Cargo.lock validation, and
Linux tests for `bilin`/`bilin-core`, including `three_node`. Full mode
also checks each `bilin` feature except `macos-ne`.

Linux tests run on one runner with 32 concurrent nextest processes. Network
integration tests use separate namespaces and bridges for each test. Subnet
proxy cases run at most eight at a time. Tests that use host-wide network state,
measure bandwidth, or make time-sensitive connectivity assertions run first,
exclusively, before the parallel tests.

These four workflows skip build/test jobs for draft PRs. Path and duplicate
content checks can skip ordinary builds and Rust jobs. The result jobs are
`core-result`, `gui-result`, `mobile-result`, and `test`.

PR dashboard builds run only for frontend, Core configuration, schema, or
build configuration changes.
Core checks run independently; Web embedding is compiled only in full mode.
Test skips the frontend build in ordinary mode; its test archive builds
only Rust packages.

PR runs can upload the test archive for reuse within CI. The dashboard is
uploaded only in full mode.
Core binaries, GUI installers, APKs, and Magisk artifacts require full mode.
Compilation outputs are cached. Ordinary pushes do not cancel full builds.

## Nightly

Start manually on `main`. Check Core,
GUI, Mobile, and Test separately. Compare main HEAD with that workflow's latest
successful `workflow_dispatch` run: skip matching commits; dispatch otherwise.
Core's `sign_windows` is set to `false` for nightly builds.

Nightly waits for the dispatched runs, retrying failed jobs once. A second
failure or a cancelled run fails Nightly and prevents publication. The build
and wait job has a three-hour timeout. Once all four components succeed at the
same commit, Nightly updates the rolling `nightly` prerelease using that commit,
even if main advances while builds are running. If main changes between the
dispatch requests and the builds target different commits, Nightly fails and
must be started again.

Nightly waits for, retries, and publishes the builds it dispatches with `GITHUB_TOKEN`.
Set the repository Actions variable `BILIN_ENABLE_NIGHTLY_AUTOMATION` to `true` to also enable completion events for full main builds dispatched manually by a maintainer.
Nightly Release can also be started manually to publish existing main HEAD
builds without rebuilding them.

Nightly uses the same asset names and archive layout as version releases.
Core, Web, and Magisk ZIP names include the built commit's version (for example,
`bilin-linux-x86_64-v2.7.0.zip`). GUI installers and Android APKs retain their
original filenames without an extra ZIP wrapper. `SHA256SUMS` covers every
download. Asset URLs can change when the project version changes; use the
fixed `nightly` release page to find the current files.

## Manual builds and releases

Repository Write access is required. In Actions, run Core, GUI, and Mobile
separately on the PR branch to obtain full artifacts. The branch must contain
these workflow definitions. Fork PRs need a temporary branch in this repository.

Manual Core builds default to SignPath test signing (`sign_windows: true`).
Full macOS GUI builds are signed and notarized.

Release is a separate manual workflow. Its Core/GUI/Mobile run IDs must refer
to successful full builds from their respective workflows at the same commit.
It downloads their artifacts and creates a draft release.

## Other workflows

- OHOS runs independently. Pushes to `ohos/**` and manual runs can build HAR artifacts. Relevant PRs run bridge tests and check the bindings for the host and ARM64 OHOS.
- Go, JavaScript Hosts, and Nix use their own path filters.
- Docker is manual; it downloads a specified Core run's artifacts and pushes images.
