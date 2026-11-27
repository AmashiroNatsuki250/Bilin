use std::env;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // YY-Thunks collides with Rust's Windows imports when linking with lld on Linux.
    if cfg!(windows) && target_os == "windows" && (target_arch == "x86" || target_arch == "x86_64")
    {
        thunk::thunk();
    }

    tauri_build::build();
}
