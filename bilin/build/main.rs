use cfg_aliases::cfg_aliases;
use std::env;

fn link_windows_third_party(target_arch: &str) {
    let variant = match target_arch {
        "x86_64" => "x86_64",
        "x86" => "i686",
        "aarch64" => "arm64",
        _ => return,
    };

    let path = std::path::PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("third_party")
        .join(variant);
    println!("cargo:rustc-link-search=native={}", path.display());
}

fn workdir() -> Option<String> {
    if let Ok(cargo_manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        return Some(cargo_manifest_dir);
    }

    let dest = std::env::var("OUT_DIR");
    if dest.is_err() {
        return None;
    }
    let dest = dest.unwrap();

    let seperator = regex::Regex::new(r"(/target/(.+?)/build/)|(\\target\\(.+?)\\build\\)")
        .expect("Invalid regex");
    let parts = seperator.split(dest.as_str()).collect::<Vec<_>>();

    if parts.len() >= 2 {
        return Some(parts[0].to_string());
    }

    None
}

fn check_locale() {
    let workdir = workdir().unwrap_or("./".to_string());

    let locale_path = format!("{workdir}/**/locales/**/*");
    if let Ok(globs) = globwalk::glob(locale_path) {
        for entry in globs {
            if let Err(e) = entry {
                println!("cargo:i18n-error={e}");
                continue;
            }

            let entry = entry.unwrap().into_path();
            println!("cargo:rerun-if-changed={}", entry.display());
        }
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .env("TZ", "UTC")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}

fn set_git_commit_date() {
    let suffix = git_output(&["log", "-1", "--format=%cd", "--date=format-local:%Y%m%d"])
        .map(|date| format!("-{date}"))
        .unwrap_or_default();
    println!("cargo:rustc-env=BILIN_COMMIT_DATE={suffix}");

    // Refresh the date even when a new commit does not change package sources.
    let args = [
        "rev-parse",
        "--git-path",
        "HEAD",
        "--git-path",
        "refs",
        "--git-path",
        "packed-refs",
    ];
    if let Some(paths) = git_output(&args) {
        for path in paths.lines() {
            if std::path::Path::new(path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    cfg_aliases! {
        mobile: {
            any(
                target_os = "android",
                target_os = "ios",
                all(target_os = "macos", feature = "macos-ne"),
                target_env = "ohos"
            )
        }
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_os == "windows" {
        link_windows_third_party(&target_arch);
        if target_arch == "x86" || target_arch == "x86_64" {
            thunk::thunk();
        }
    }

    check_locale();
    set_git_commit_date();
    Ok(())
}
