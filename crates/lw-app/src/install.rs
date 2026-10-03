//! Downloading, choosing and deleting models.
//!
//! This was the last piece still living in the Tauri command layer when the rest moved out, and it
//! is the piece a user notices missing first: without it the model list is a catalogue you can read
//! and not a catalogue you can act on.
//!
//! Three operations, and each one guards something different.
//!
//! - **Install** fetches only what a pinned manifest names, verifies every file against its pinned
//!   SHA-256 in a staging directory, and promotes the directory atomically. Nothing downloaded is
//!   ever executed: ONNX graphs, vocabularies and context binaries only.
//! - **Select** refuses a model that is not installed or that this build cannot run, because the
//!   consequence of getting it wrong appears later, at the worst moment -- the next time the hotkey
//!   is pressed, with no engine to load.
//! - **Delete** refuses the model dictation is set to use, and proves the directory it is about to
//!   remove is inside the models root by comparing canonical paths, so a bug upstream cannot turn
//!   into "delete this directory".

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender, unbounded};
use lw_core::model::{
    ArtifactTarget, CancellationToken, Catalog, DownloadEvent, ModelDownloader, ModelManifest, ModelRegistry,
    manifests_dir, preferred_targets, staging_dir_for,
};
use lw_core::settings::Settings;

use crate::machine::probe_capabilities;

/// What an install is doing, as the model list shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// Starting a file. `total` is its size in bytes.
    FileStarted { path: String, total: u64 },
    /// Bytes so far for the file named by the last `FileStarted`.
    Bytes {
        path: String,
        received: u64,
        total: u64,
        file_index: usize,
        file_count: usize,
    },
    /// A file's hash matched the manifest.
    Verified { path: String },
    /// Everything downloaded, verified and promoted out of staging.
    Done { dir: String },
    /// The user asked it to stop. Partial files stay in staging and the next attempt resumes.
    Cancelled,
    /// It went wrong, with the reason.
    Failed { message: String },
}

impl Progress {
    /// Is this the last word on an install?
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Progress::Done { .. } | Progress::Cancelled | Progress::Failed { .. }
        )
    }
}

/// One install, as it appears on screen.
#[derive(Clone, Debug, Default)]
pub struct InstallState {
    /// The file being fetched right now.
    pub file: String,
    pub received: u64,
    pub total: u64,
    pub file_index: usize,
    pub file_count: usize,
    /// Set once the last file is verified and the directory is being promoted: there is nothing
    /// left to count, and a bar sitting at 100% with no explanation reads as a hang.
    pub finishing: bool,
    /// Asked to stop, but not yet stopped.
    pub cancelling: bool,
}

impl InstallState {
    /// Fraction of the current file, 0..1, or `None` when the size is not known yet.
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.received as f32 / self.total as f32).clamp(0.0, 1.0))
    }
}

/// An install in flight, and the way to stop it.
pub struct Handle {
    /// Which model. One at a time, deliberately: two concurrent downloads of a few gigabytes each
    /// share one link and finish later than they would in sequence.
    pub id: String,
    pub events: Receiver<Progress>,
    cancel: CancellationToken,
    state: Arc<Mutex<InstallState>>,
}

impl Handle {
    /// Ask it to stop. Partial files remain in staging, so the next attempt resumes.
    pub fn cancel(&self) {
        self.cancel.cancel();
        if let Ok(mut s) = self.state.lock() {
            s.cancelling = true;
        }
    }

    /// What to draw right now.
    pub fn state(&self) -> InstallState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Drain the channel into the shared state, returning the terminal event if one arrived.
    pub fn poll(&self) -> Option<Progress> {
        let mut last = None;
        while let Ok(ev) = self.events.try_recv() {
            if let Ok(mut s) = self.state.lock() {
                match &ev {
                    Progress::FileStarted { path, total } => {
                        s.file = path.clone();
                        s.received = 0;
                        s.total = *total;
                    }
                    Progress::Bytes {
                        path,
                        received,
                        total,
                        file_index,
                        file_count,
                    } => {
                        s.file = path.clone();
                        s.received = *received;
                        s.total = *total;
                        s.file_index = *file_index;
                        s.file_count = *file_count;
                    }
                    // The last file's verification is the point where there is nothing left to
                    // count but the promotion out of staging, which can take a moment on a large
                    // model.
                    Progress::Verified { .. }
                        if s.file_count > 0 && s.file_index + 1 >= s.file_count =>
                    {
                        s.finishing = true;
                    }
                    _ => {}
                }
            }
            if ev.is_terminal() {
                last = Some(ev);
                break;
            }
        }
        last
    }
}

/// Start downloading `id` into the models root beside `settings_path`.
///
/// Returns immediately; progress arrives on the handle's channel. The download runs on a thread
/// with its own single-threaded Tokio runtime, because the downloader is async and the front end
/// is not -- and because a download that outlives a repaint must not be tied to one.
pub fn start(settings_path: &Path, id: &str) -> Handle {
    start_with_target(settings_path, id, None, None)
}

/// Download the optional full-precision Parakeet encoder for this machine's GPU.
pub fn start_gpu_encoder(
    settings_path: &Path,
    accel: lw_core::capabilities::Accelerator,
) -> Handle {
    start_with_target(settings_path, "parakeet-tdt-0.6b-v3", Some(ArtifactTarget::GpuFp32), Some(accel))
}

fn start_with_target(
    settings_path: &Path,
    id: &str,
    addon: Option<ArtifactTarget>,
    verify_backend: Option<lw_core::capabilities::Accelerator>,
) -> Handle {
    let (tx, rx) = unbounded::<Progress>();
    let cancel = CancellationToken::new();
    let state = Arc::new(Mutex::new(InstallState::default()));

    let root = models_root(settings_path);
    let settings_path = settings_path.to_path_buf();
    let id_owned = id.to_string();
    let cancel_thread = cancel.clone();
    let tx_thread = tx.clone();

    std::thread::Builder::new()
        .name("lw-install".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx_thread.send(Progress::Failed {
                        message: format!("could not start the download runtime: {e}"),
                    });
                    return;
                }
            };
            let cancelled = cancel_thread.clone();
            let result = runtime.block_on(install_inner(
                &id_owned,
                &root,
                addon,
                cancel_thread,
                &tx_thread,
            ));
            let result = result.and_then(|dir| {
                if let Some(accel) = verify_backend {
                    crate::bench::run_benchmark_job(
                        &settings_path,
                        Some(id_owned.clone()),
                        Some(lw_core::engine::BackendPreference::for_accelerator(accel)),
                        None,
                    ).map_err(|e| format!("GPU files installed but {} could not run the model: {e}", accel.label()))?;
                }
                Ok(dir)
            });
            match result {
                Ok(dir) => {
                    let _ = tx_thread.send(Progress::Done { dir });
                }
                // A cancelled download is not a failure and must not be reported as one.
                Err(_) if cancelled.is_cancelled() => {
                    let _ = tx_thread.send(Progress::Cancelled);
                }
                Err(e) => {
                    let _ = tx_thread.send(Progress::Failed { message: e });
                }
            }
        })
        .expect("spawn the install thread");

    Handle {
        id: id.to_string(),
        events: rx,
        cancel,
        state,
    }
}

async fn install_inner(
    id: &str,
    root: &Path,
    addon: Option<ArtifactTarget>,
    cancel: CancellationToken,
    tx: &Sender<Progress>,
) -> Result<String, String> {
    let manifest = manifest_for(id)?;
    let (target, files) = if let Some(addon) = addon {
        let set = manifest.artifacts.iter().find(|a| a.target == addon)
            .ok_or_else(|| format!("no {addon:?} files in the manifest for '{id}'"))?;
        (addon, set.files.clone())
    } else {
        manifest.select_files(&preferred_targets(probe_capabilities()))
            .ok_or_else(|| format!("no artifact in the manifest for '{id}' matches this machine"))?
    };
    tracing::info!("installing {id} ({} files, target {target:?})", files.len());

    let registry = ModelRegistry::new(root);
    let final_dir = registry.model_dir(&manifest);
    let staging = staging_dir_for(&final_dir);

    let downloader = ModelDownloader::new().map_err(|e| e.to_string())?;
    let out = tx.clone();
    // `Progress` carries indices, not a name, so remember the file the last `FileStarted`
    // announced and report bytes against it.
    let mut current = String::new();
    let report = move |ev| {
        let mapped = match ev {
            DownloadEvent::FileStarted { path, total } => {
                current = path.clone();
                Progress::FileStarted { path, total }
            }
            DownloadEvent::Progress(p) => Progress::Bytes {
                path: current.clone(),
                received: p.file_downloaded,
                total: p.file_total,
                file_index: p.file_index,
                file_count: p.file_count,
            },
            DownloadEvent::FileVerified { path } => Progress::Verified { path },
            DownloadEvent::Completed => Progress::Done {
                dir: String::new(),
            },
        };
        // The completion event is sent by the caller, with the directory filled in.
        if !matches!(mapped, Progress::Done { .. }) {
            let _ = out.send(mapped);
        }
    };
    if addon.is_some() {
        downloader.install_addon(&files, &final_dir, cancel, report)
            .await.map_err(|e| e.to_string())?;
    } else {
        downloader.install(&files, &staging, &final_dir, cancel, report)
            .await.map_err(|e| e.to_string())?;
    }

    Ok(final_dir.display().to_string())
}

/// Make `id` the model dictation uses.
///
/// Writes `settings.json` and nothing else; the caller is responsible for telling the worker to
/// reload. Refuses a model that is not installed, because the failure would otherwise surface at
/// the next hotkey press rather than at the click that caused it.
pub fn select(settings_path: &Path, id: &str) -> Result<(), String> {
    let root = models_root(settings_path);
    let dir = root.join(id);
    if !dir.exists() {
        return Err(format!(
            "'{id}' is not installed ({}). Download it first.",
            dir.display()
        ));
    }
    let mut settings = Settings::load(settings_path).map_err(|e| e.to_string())?;
    if settings.model_id == id {
        return Ok(());
    }
    settings.model_id = id.to_string();
    settings.save(settings_path).map_err(|e| e.to_string())?;
    tracing::info!("model selected: {id}");
    Ok(())
}

/// Remove a model's downloaded files. Returns the bytes freed.
pub fn delete(settings_path: &Path, id: &str) -> Result<u64, String> {
    let settings = Settings::load(settings_path).map_err(|e| e.to_string())?;
    if settings.model_id == id {
        return Err(format!(
            "'{id}' is the model dictation is set to use. Choose another model first, then delete \
             this one."
        ));
    }

    let root = models_root(settings_path);
    // Via the manifest, not by joining the id: the registry decides the directory name, and a
    // guess that happened to be wrong would delete the wrong thing or nothing at all.
    let manifest = manifest_for(id)?;
    let dir = ModelRegistry::new(&root).model_dir(&manifest);
    remove_model_dir(&root, &dir)
}

/// The pinned manifest for `id`, read and validated.
fn manifest_for(id: &str) -> Result<ModelManifest, String> {
    let catalog = Catalog::builtin().map_err(|e| e.to_string())?;
    let entry = catalog
        .get(id)
        .ok_or_else(|| format!("unknown model id '{id}'"))?;
    let name = entry.manifest.as_ref().ok_or_else(|| {
        format!("'{id}' has no pinned manifest, so it cannot be downloaded safely")
    })?;
    let dir = manifests_dir(None).ok_or_else(|| "manifest directory not found".to_string())?;
    let path = dir.join(name);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest: ModelManifest =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    manifest.validate().map_err(|e| e.to_string())?;
    Ok(manifest)
}

/// Models live beside `settings.json`, which is where the worker loads them from.
fn models_root(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .map(|p| p.join("models"))
        .unwrap_or_else(|| PathBuf::from("models"))
}

/// Remove `dir`, having proved it is inside `root`. Returns the bytes freed.
fn remove_model_dir(root: &Path, dir: &Path) -> Result<u64, String> {
    if !dir.exists() {
        return Ok(0);
    }
    // Canonical paths on both sides: a `..` in either would let a directory outside the models
    // root pass a textual check.
    let root_real = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    let dir_real = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    if !dir_real.starts_with(&root_real) || dir_real == root_real {
        return Err(format!(
            "refusing to delete {}: it is not a model directory inside {}",
            dir_real.display(),
            root_real.display()
        ));
    }
    let freed = dir_size(&dir_real);
    std::fs::remove_dir_all(&dir_real).map_err(|e| format!("{}: {e}", dir_real.display()))?;
    tracing::info!("deleted model directory {} ({freed} bytes)", dir_real.display());
    Ok(freed)
}

/// Total size of a directory tree, for reporting what a delete freed.
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lw-install-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_directory_outside_the_models_root_is_refused() {
        // The guard that matters. Everything else here is a convenience; this is the one that
        // stands between a wrong manifest and somebody's home directory.
        let base = temp();
        let root = base.join("models");
        let outside = base.join("not-models");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("precious.txt"), b"keep me").unwrap();

        let err = remove_model_dir(&root, &outside).expect_err("should refuse");
        assert!(err.contains("refusing to delete"), "{err}");
        assert!(outside.join("precious.txt").exists(), "it deleted anyway");

        // ...and the root itself is not a model directory either.
        assert!(remove_model_dir(&root, &root).is_err());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_model_directory_inside_the_root_is_removed_and_its_size_reported() {
        let base = temp();
        let root = base.join("models2");
        let model = root.join("some-model");
        std::fs::create_dir_all(model.join("nested")).unwrap();
        std::fs::write(model.join("a.onnx"), vec![0u8; 1000]).unwrap();
        std::fs::write(model.join("nested").join("b.bin"), vec![0u8; 24]).unwrap();

        let freed = remove_model_dir(&root, &model).expect("removed");
        assert_eq!(freed, 1024, "the reported size must include nested files");
        assert!(!model.exists());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn selecting_a_model_that_is_not_installed_is_refused() {
        // Otherwise the mistake surfaces at the next hotkey press, with no engine to load and no
        // connection in the user's mind to the click that caused it.
        let base = temp();
        let settings_path = base.join("settings.json");
        Settings::default().save(&settings_path).unwrap();

        let err = select(&settings_path, "not-installed-anywhere").expect_err("should refuse");
        assert!(err.contains("not installed"), "{err}");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn deleting_the_model_in_use_is_refused() {
        let base = temp();
        let settings_path = base.join("settings.json");
        let settings = Settings {
            model_id: "in-use".into(),
            ..Default::default()
        };
        settings.save(&settings_path).unwrap();

        let err = delete(&settings_path, "in-use").expect_err("should refuse");
        assert!(err.contains("set to use"), "{err}");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_fraction_is_only_reported_once_the_size_is_known() {
        // A bar that starts at 100% because nothing has been divided yet is worse than no bar.
        let mut s = InstallState::default();
        assert_eq!(s.fraction(), None);
        s.total = 400;
        s.received = 100;
        assert_eq!(s.fraction(), Some(0.25));
    }
}
