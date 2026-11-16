//! Optional local Hy-MT2 translation. Setup is explicit; dictation never downloads a model.

mod install;
mod process;

pub use install::{Setup, SetupProgress, download_size, is_ready, runtime_backend, start_setup};
pub use process::{Translator, stop_all};

use std::path::{Path, PathBuf};

use lw_core::model::FileEntry;
use serde::Deserialize;

#[derive(Deserialize)]
struct Package {
    os: String,
    arch: String,
    backend: String,
    file: FileEntry,
}

#[derive(Deserialize)]
struct Manifest {
    runtime_version: String,
    model: FileEntry,
    packages: Vec<Package>,
}

fn manifest() -> Manifest {
    serde_json::from_str(include_str!("packages.json")).expect("embedded translator manifest")
}

fn root(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("translation")
}

fn packages(backend: &str) -> Result<Vec<FileEntry>, String> {
    let files: Vec<_> = manifest()
        .packages
        .into_iter()
        .filter(|p| p.os == std::env::consts::OS && p.arch == std::env::consts::ARCH && p.backend == backend)
        .map(|p| p.file)
        .collect();
    if files.is_empty() {
        Err(format!(
            "Local translation runtime is not packaged for {} / {} / {backend}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    } else {
        Ok(files)
    }
}

fn runtime_dir(settings_path: &Path, backend: &str) -> Option<PathBuf> {
    let base = root(settings_path);
    let name: String =
        serde_json::from_slice(&std::fs::read(base.join(format!("active-{backend}.json"))).ok()?).ok()?;
    if name.is_empty()
        || Path::new(&name)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    let dir = base.join(name);
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("installed.json")).ok()?).ok()?;
    if marker["runtime_version"] != manifest().runtime_version || marker["backend"] != backend {
        return None;
    }
    Some(dir)
}

fn server_in(dir: &Path) -> Option<PathBuf> {
    let filename = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    let mut todo = vec![dir.to_path_buf()];
    let mut seen = 0;
    while let Some(path) = todo.pop() {
        for entry in std::fs::read_dir(path).ok()? {
            let entry = entry.ok()?;
            seen += 1;
            if seen > 4096 {
                return None;
            }
            let ty = entry.file_type().ok()?;
            if ty.is_dir() {
                todo.push(entry.path());
            } else if ty.is_file() && entry.file_name() == filename {
                return Some(entry.path());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_downloads_have_pinned_https_urls_hashes_and_safe_paths() {
        let m = manifest();
        for f in std::iter::once(&m.model).chain(m.packages.iter().map(|p| &p.file)) {
            assert!(f.path_is_safe());
            assert!(
                f.url.starts_with("https://huggingface.co/")
                    || f.url
                        .starts_with("https://github.com/ggml-org/llama.cpp/releases/download/b12420/")
            );
            assert_eq!(f.sha256.len(), 64);
            assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(f.bytes > 0);
        }
    }
}
