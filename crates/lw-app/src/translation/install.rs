use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

use crossbeam_channel::{Receiver, bounded};
use lw_core::{
    model::{CancellationToken, DownloadEvent, ModelDownloader},
    settings::TranslationBackend,
};

use super::{manifest, packages, root, runtime_dir, server_in};

/// Choose a runtime without confusing an ONNX provider with a llama.cpp accelerator.
pub fn runtime_backend(preference: TranslationBackend) -> &'static str {
    match preference {
        TranslationBackend::Cpu => "cpu",
        TranslationBackend::Cuda => "cuda",
        TranslationBackend::Auto => {
            if cfg!(all(windows, target_arch = "x86_64"))
                && lw_platform::caps::physical_gpu_descriptions()
                    .iter()
                    .any(|s| s.to_ascii_lowercase().contains("nvidia"))
            {
                "cuda"
            } else {
                "cpu"
            }
        }
    }
}

/// Whether the requested runtime and verified-install model are present.
pub fn is_ready(settings_path: &Path, preference: TranslationBackend) -> bool {
    let backend = runtime_backend(preference);
    runtime_dir(settings_path, backend)
        .and_then(|d| server_in(&d))
        .is_some()
        && fs::metadata(root(settings_path).join(manifest().model.path))
            .is_ok_and(|m| m.len() == manifest().model.bytes)
}

/// Pinned download size, including the optional runtime.
pub fn download_size(preference: TranslationBackend) -> Result<u64, String> {
    Ok(manifest().model.bytes
        + packages(runtime_backend(preference))?
            .iter()
            .map(|f| f.bytes)
            .sum::<u64>())
}

/// Download progress for the Settings card.
#[derive(Clone, Debug, Default)]
pub struct SetupProgress {
    /// User-facing phase description.
    pub message: String,
    /// Overall download fraction, when known.
    pub fraction: Option<f32>,
}

/// An explicit setup operation; dropping it cancels outstanding downloading.
pub struct Setup {
    result: Receiver<Result<(), String>>,
    state: Arc<Mutex<SetupProgress>>,
    cancel: CancellationToken,
}

impl Setup {
    /// Latest progress, without blocking the GUI.
    pub fn progress(&self) -> SetupProgress {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
    /// Completed setup or failure, once available.
    pub fn poll(&self) -> Option<Result<(), String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(crossbeam_channel::TryRecvError::Empty) => None,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                Some(Err("Translation setup stopped unexpectedly.".into()))
            }
        }
    }
    /// Stop at the next downloader/extractor cancellation point.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Download only when the user explicitly asks. All bytes are verified before extraction/execution.
pub fn start_setup(settings_path: &Path, preference: TranslationBackend) -> Setup {
    let (tx, result) = bounded(1);
    let state = Arc::new(Mutex::new(SetupProgress {
        message: "Preparing local translator...".into(),
        fraction: None,
    }));
    let cancel = CancellationToken::new();
    let path = settings_path.to_path_buf();
    let progress = state.clone();
    let stop = cancel.clone();
    std::thread::spawn(move || {
        let _ = tx.send(install(&path, preference, &progress, stop));
    });
    Setup {
        result,
        state,
        cancel,
    }
}

fn update(state: &Mutex<SetupProgress>, message: impl Into<String>, fraction: Option<f32>) {
    if let Ok(mut s) = state.lock() {
        *s = SetupProgress {
            message: message.into(),
            fraction,
        };
    }
}

fn install(
    settings_path: &Path,
    preference: TranslationBackend,
    state: &Mutex<SetupProgress>,
    stop: CancellationToken,
) -> Result<(), String> {
    let backend = runtime_backend(preference);
    let m = manifest();
    let runtime_files = packages(backend)?;
    let base = root(settings_path);
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let mut files = vec![m.model.clone()];
    files.extend(runtime_files.iter().cloned());
    let total = files.iter().map(|f| f.bytes).sum::<u64>();
    let mut completed = 0;
    // ModelDownloader's add-on path resumes partial files and verifies already cached bytes.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        ModelDownloader::new()
            .map_err(|e| e.to_string())?
            .install_addon(&files, &base, stop.clone(), |event| match event {
                DownloadEvent::FileStarted { path, .. } => update(
                    state,
                    format!("Downloading {path}"),
                    Some(completed as f32 / total as f32),
                ),
                DownloadEvent::Progress(p) => update(
                    state,
                    "Downloading local translator...",
                    Some((completed + p.file_downloaded) as f32 / total as f32),
                ),
                DownloadEvent::FileVerified { path } => {
                    completed += files.iter().find(|f| f.path == path).map_or(0, |f| f.bytes);
                    update(
                        state,
                        format!("Verified {path}"),
                        Some(completed as f32 / total as f32),
                    );
                }
                DownloadEvent::Completed => {}
            })
            .await
            .map_err(|e| e.to_string())
    })?;
    if stop.is_cancelled() {
        return Err("Translation setup cancelled.".into());
    }
    let name = format!("runtime-{}-{backend}-{}", m.runtime_version, uuid::Uuid::new_v4());
    let stage = base.join(format!(".{name}.staging"));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    update(state, "Installing verified translation runtime...", None);
    let extraction = (|| {
        for f in &runtime_files {
            if stop.is_cancelled() {
                return Err("Translation setup cancelled.".to_string());
            }
            extract(&base.join(&f.path), &stage, &stop)?;
        }
        let server = server_in(&stage).ok_or("Translation package has no llama-server executable")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        let mut command = std::process::Command::new(&server);
        command
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("Cannot start translation runtime: {e}"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    return Err("Translation runtime could not load its libraries. Choose CPU or check the NVIDIA driver.".into());
                }
                break;
            }
            if stop.is_cancelled() || std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Translation runtime check stopped or timed out.".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        fs::write(
            stage.join("installed.json"),
            serde_json::to_vec(&serde_json::json!({"runtime_version":m.runtime_version,"backend":backend}))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(error) = extraction {
        // This is a newly created, owned staging directory, never a user-selected path.
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    fs::rename(&stage, base.join(&name)).map_err(|e| e.to_string())?;
    let pending = base.join(format!(".active-{backend}-{}.json", uuid::Uuid::new_v4()));
    fs::write(&pending, serde_json::to_vec(&name).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(pending, base.join(format!("active-{backend}.json"))).map_err(|e| e.to_string())?;
    update(state, "Local translator is ready.", Some(1.0));
    Ok(())
}

fn safe_relative(path: &Path) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(v) if !v.to_string_lossy().contains([':', '\\']) => out.push(v),
            Component::CurDir => {}
            _ => return Err("Unsafe path in translation runtime archive".into()),
        }
    }
    Ok(out)
}

fn extract(archive: &Path, destination: &Path, cancel: &CancellationToken) -> Result<(), String> {
    let mut total = 0u64;
    if archive.extension().is_some_and(|s| s == "zip") {
        let mut zip = zip::ZipArchive::new(fs::File::open(archive).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if zip.len() > 4096 {
            return Err("Too many runtime archive entries".into());
        }
        for index in 0..zip.len() {
            if cancel.is_cancelled() {
                return Err("Translation setup cancelled.".into());
            }
            let entry = zip.by_index(index).map_err(|e| e.to_string())?;
            let rel = safe_relative(&entry.enclosed_name().ok_or("Unsafe runtime archive path")?)?;
            if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err("Links in a ZIP runtime are not supported".into());
            }
            total = total.saturating_add(entry.size());
            if total > 4 * 1024 * 1024 * 1024 {
                return Err("Runtime archive is too large".into());
            }
            let target = destination.join(rel);
            if entry.is_dir() {
                fs::create_dir_all(target).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut file = fs::File::create(target).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry.take(4 * 1024 * 1024 * 1024 + 1), &mut file)
                .map_err(|e| e.to_string())?;
        }
    } else {
        let reader = flate2::read::GzDecoder::new(fs::File::open(archive).map_err(|e| e.to_string())?);
        let mut tar = tar::Archive::new(reader);
        let mut links = Vec::new();
        for (index, entry) in tar.entries().map_err(|e| e.to_string())?.enumerate() {
            if index >= 4096 || cancel.is_cancelled() {
                return Err("Runtime extraction stopped".into());
            }
            let mut entry = entry.map_err(|e| e.to_string())?;
            let rel = safe_relative(&entry.path().map_err(|e| e.to_string())?)?;
            let target = destination.join(&rel);
            let kind = entry.header().entry_type();
            if kind.is_dir() {
                fs::create_dir_all(target).map_err(|e| e.to_string())?;
                continue;
            }
            if kind.is_symlink() || kind.is_hard_link() {
                let link = entry
                    .link_name()
                    .map_err(|e| e.to_string())?
                    .ok_or("Missing runtime link target")?;
                let source = if kind.is_symlink() {
                    rel.parent().unwrap_or(Path::new("")).join(link)
                } else {
                    link.into_owned()
                };
                links.push((target, destination.join(safe_relative(&source)?)));
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            total = total.saturating_add(entry.size());
            if total > 4 * 1024 * 1024 * 1024 {
                return Err("Runtime archive is too large".into());
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut file = fs::File::create(target).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut file).map_err(|e| e.to_string())?;
        }
        // Materialize in-archive library aliases as regular files; never create filesystem links.
        for _ in 0..16 {
            if links.is_empty() {
                break;
            }
            let mut pending = Vec::new();
            for (target, source) in links {
                if source.is_file() {
                    fs::copy(source, target).map_err(|e| e.to_string())?;
                } else {
                    pending.push((target, source));
                }
            }
            links = pending;
        }
        if !links.is_empty() {
            return Err("Runtime archive contains unresolved library aliases".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_archive_paths_that_can_escape_or_name_a_windows_drive() {
        for path in [
            "../escape",
            "/escape",
            "C:/escape",
            "nested/../../escape",
            "nested\\..\\..\\escape",
        ] {
            assert!(safe_relative(Path::new(path)).is_err(), "accepted {path}");
        }
        assert_eq!(
            safe_relative(Path::new("bin/llama-server")).unwrap(),
            Path::new("bin/llama-server")
        );
    }

    #[test]
    fn zip_extraction_rejects_traversal_without_touching_outside_files() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("runtime");
        fs::create_dir(&destination).unwrap();
        let outside = dir.path().join("keep.txt");
        fs::write(&outside, "keep this").unwrap();
        let archive = dir.path().join("unsafe.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file("../keep.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"corrupted").unwrap();
        zip.finish().unwrap();
        assert!(extract(&archive, &destination, &CancellationToken::new()).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "keep this");
    }
}
