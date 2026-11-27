# Vendored Rust dependencies

These sources are pinned to the revisions used by the original lockfile. Upstream copyright and license files are kept alongside the source. Cargo resolves the six repositories locally.

| Directory | Original repository | Revision |
|---|---|---|
| `kcp-sys` | [EasyTier/kcp-sys](https://github.com/EasyTier/kcp-sys) | `268533568d734ae89dc89603078da3ca522effe1` |
| `kcp-sys/kcp` | [skywind3000/kcp](https://github.com/skywind3000/kcp) | `32da082e529a26730aea3eb19922f80634ee6dea` |
| `rust-tun` | [EasyTier/rust-tun](https://github.com/EasyTier/rust-tun) | `43621f70b02c47086b40d34fdde997719668a6c0` |
| `service-manager-rs` | [EasyTier/service-manager-rs](https://github.com/EasyTier/service-manager-rs) | `5eb28f7a686858eea4f4933534ed989d3b71dc2a` |
| `thunk` | [EasyTier/thunk](https://github.com/EasyTier/thunk) | `cbbeec75a66b7b3cf0824ae890d9d06bcfb9d1f3` |
| `tokio-websockets` | [EasyTier/tokio-websockets](https://github.com/EasyTier/tokio-websockets) | `dc9771c7c215882349c3cb328877550a3593df21` |
| `windivert-rust` | [EasyTier/windivert-rust](https://github.com/EasyTier/windivert-rust) | `adcc56d1550f7b5377ec2b3429f413ee24a77375` |
| `windivert-rust/windivert-sys/vendor` | [EasyTier/WinDivert](https://github.com/EasyTier/WinDivert) | `b90fad29446667c6230accd078e91a77074b7788` |

The WinDivert MSVC static build uses portable source and include paths for clang-cl cross-compilation.
