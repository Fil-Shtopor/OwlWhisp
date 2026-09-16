use std::path::{Path, PathBuf};

fn main() {
    stage_runtime_resources();
    tauri_build::build()
}

/// Copy the staged execution-provider libraries for this platform into `src-tauri/runtime/`, which
/// `tauri.conf.json` bundles as a resource.
///
/// Without this an installer ships an application with no ONNX Runtime beside it: the app would
/// install cleanly and then fail to load a model, which is the worst possible time to find out.
/// Doing it in `build.rs` rather than a wrapper script means a plain `cargo tauri build` is
/// self-contained, including in CI.
///
/// Missing sources are **not** an error. A developer building without a staged runtime still gets
/// a working debug build (the app looks for `runtime/` beside the executable and up the tree), and
/// failing the build here would make `cargo check` depend on a 200 MB download.
fn stage_runtime_resources() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest.join("..").join("..");
    let platform = platform_dir();
    let src = repo.join("runtime").join(platform);
    let dest = manifest.join("runtime");

    println!("cargo:rerun-if-changed={}", src.display());

    if !src.is_dir() {
        println!(
            "cargo:warning=no staged runtime at {} - the bundle will not include ONNX Runtime. \
             Run scripts/runtime/fetch-runtime.ps1 before packaging.",
            src.display()
        );
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&dest) {
        println!("cargo:warning=cannot create {}: {e}", dest.display());
        return;
    }
    let Ok(entries) = std::fs::read_dir(&src) else {
        return;
    };
    let mut copied = 0usize;
    for entry in entries.flatten() {
        let from = entry.path();
        if !from.is_file() {
            continue;
        }
        let to = dest.join(entry.file_name());
        // These are hundreds of megabytes; only copy what actually changed.
        if is_up_to_date(&from, &to) {
            continue;
        }
        match std::fs::copy(&from, &to) {
            Ok(_) => copied += 1,
            Err(e) => println!("cargo:warning=copy {}: {e}", from.display()),
        }
    }
    if copied > 0 {
        println!(
            "cargo:warning=staged {copied} runtime file(s) into {}",
            dest.display()
        );
    }
}

/// Same layout the runtime loader expects (`lw_ort::runtime_platform_dir`).
fn platform_dir() -> &'static str {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    match (os.as_str(), arch.as_str()) {
        ("windows", "aarch64") => "win-arm64",
        ("windows", _) => "win-x64",
        ("macos", "aarch64") => "osx-arm64",
        ("macos", _) => "osx-x64",
        (_, "aarch64") => "linux-arm64",
        _ => "linux-x64",
    }
}

/// Whether `to` already matches `from` by size and is no older.
fn is_up_to_date(from: &Path, to: &Path) -> bool {
    let (Ok(a), Ok(b)) = (from.metadata(), to.metadata()) else {
        return false;
    };
    if a.len() != b.len() {
        return false;
    }
    match (a.modified(), b.modified()) {
        (Ok(ma), Ok(mb)) => mb >= ma,
        _ => false,
    }
}
