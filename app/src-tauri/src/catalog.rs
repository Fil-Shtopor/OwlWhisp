//! IPC for the model catalog, model installation and on-machine benchmarking.
//!
//! Everything here observes one rule the rest of the project observes: an **estimate** and a
//! **measurement** are different things and are never conflated. `list_models` returns
//! `estimated_rtf` (computed from a speed tier and detected hardware) alongside
//! `measured_reference` (a real run, tagged with the machine it happened on), and the frontend
//! is required to label them differently.
//!
//! Contract (see `app/frontend/src/ipc.ts` for the TypeScript mirror):
//! - `get_capabilities() -> Capabilities`
//! - `list_models() -> ModelCatalog`
//! - `install_model(id, channel)` streaming `InstallEvent`s
//! - `cancel_install(id)`
//! - `run_benchmark(modelId?, backend?) -> BenchReport`

use std::path::PathBuf;

use lw_core::capabilities::Capabilities;
use lw_core::engine::BackendPreference;
use lw_core::model::{
    CancellationToken, Catalog, DownloadEvent, ModelDownloader, ModelManifest, ModelRegistry,
    default_models_root, manifests_dir, preferred_targets, staging_dir_for,
};
use serde_json::{Value, json};
use tauri::State;
use tauri::ipc::Channel;

use crate::state::AppState;
use lw_app::bench::BenchReport;
use lw_app::machine::probe_capabilities;

use crate::worker::WorkerCmd;

/// Models live beside `settings.json`, which is also where the worker loads them from.
///
/// Deliberately *not* [`default_models_root`]: the app owns its Tauri app-data directory, and
/// the two must not disagree about what is installed. `LW_MODELS_ROOT` points the CLI here.
fn models_root(state: &AppState) -> PathBuf {
    state
        .settings_path
        .parent()
        .map(|p| p.join("models"))
        .unwrap_or_else(default_models_root)
}

/// Detected hardware **with ONNX Runtime provider availability filled in**, probed once.
///
/// `lw_platform::caps::detect()` deliberately leaves `providers.*` false: it reads the OS and the
/// driver store, and cannot know whether an execution provider actually loads. Using it raw would
/// make the app report "QNN EP unavailable" on a machine whose NPU works, refuse to prefer NPU
/// artifacts, and never recommend the NPU path.
///
/// Detect hardware and report it, including whether the NPU is merely present or actually usable.
///
/// Probing loads `onnxruntime.dll`, so this runs on a blocking thread.
#[tauri::command]
pub async fn get_capabilities() -> Value {
    tauri::async_runtime::spawn_blocking(|| capabilities_json(probe_capabilities()))
        .await
        .unwrap_or_else(|e| json!({ "error": format!("capability probe failed: {e}") }))
}

fn capabilities_json(caps: &Capabilities) -> Value {
    json!({
        "machine": caps.summary(),
        "cpu": caps.cpu_brand,
        "cores": caps.cpu_cores,
        "os": caps.os,
        "arch": caps.arch,
        "npu_present": caps.npu.present,
        "npu_label": caps.npu.htp_arch.map(|a| format!("Hexagon V{}", a.num())),
        // Present is not the same as usable: a driver package can be installed while the QNN
        // execution provider still fails to load.
        "npu_usable": caps.npu.present && caps.providers.qnn,
    })
}

/// Every accelerator this build knows about, with what it actually looks like here.
///
/// `present`, `registered` and `devices` are kept apart on purpose: they answer different
/// questions and routinely disagree. `usable` is the only one that means acceleration, and it is
/// the one the settings UI must gate on.
pub fn accelerators_json() -> Value {
    let Ok(rt) = lw_ort::OrtRuntime::auto() else {
        // Without a runtime we know nothing about providers -- say so rather than reporting
        // everything as unavailable, which would look like a hardware verdict.
        return json!({
            "error": "ONNX Runtime not found; accelerator availability is unknown",
            "items": [],
        });
    };
    let items: Vec<Value> = rt
        .probe_accelerators()
        .into_iter()
        .map(|st| {
            json!({
                "id": st.accel.id(),
                "label": st.accel.label(),
                "kind": st.accel.kind(),
                "kind_label": st.accel.kind().label(),
                "vendor": st.accel.vendor(),
                "library": st.accel.library_file(),
                "needs_dedicated_artifact": st.accel.needs_dedicated_artifact(),
                "present": st.present,
                "registered": st.registered,
                "devices": st.devices,
                "usable": st.usable(),
                "detail": st.explain(),
                // The preference value the settings UI should save to pin this one exactly.
                "preference": lw_core::engine::BackendPreference::for_accelerator(st.accel),
            })
        })
        .collect();
    json!({ "error": Value::Null, "items": items })
}

/// The accelerators on this machine, for the settings UI and diagnostics.
#[tauri::command]
pub async fn list_accelerators() -> Value {
    tauri::async_runtime::spawn_blocking(accelerators_json)
        .await
        .unwrap_or_else(|e| json!({ "error": format!("accelerator probe failed: {e}"), "items": [] }))
}


/// The catalog, matched against this machine and this disk.
#[tauri::command]
pub async fn list_models(state: State<'_, AppState>) -> Result<Value, String> {
    let root = models_root(&state);
    tauri::async_runtime::spawn_blocking(move || build_catalog_json(root))
        .await
        .map_err(|e| format!("catalog task failed: {e}"))?
}

/// The catalog as JSON for the web front end.
///
/// A thin wrapper now: the view is built and typed in `lw_app::catalog`, and this only serializes
/// it. The field names there are the same as the keys that used to be written here by hand, so the
/// payload is unchanged -- which is what lets the native front end be built against the same types
/// while the web one keeps working.
fn build_catalog_json(root: PathBuf) -> Result<Value, String> {
    let view = lw_app::catalog::build(&root)?;
    serde_json::to_value(view).map_err(|e| e.to_string())
}

/// Download and SHA-256-verify a model's pinned files, streaming progress to the frontend.
///
/// Nothing downloaded is ever executed: only ONNX graphs, vocabularies and context binaries are
/// fetched, and each is verified against a pinned hash before being promoted out of staging.
#[tauri::command]
pub async fn install_model(
    state: State<'_, AppState>,
    id: String,
    channel: Channel<Value>,
) -> Result<(), String> {
    let root = models_root(&state);
    let cancel = CancellationToken::new();
    state.installs.lock().insert(id.clone(), cancel.clone());

    let result = install_inner(&id, root, cancel.clone(), &channel).await;
    state.installs.lock().remove(&id);

    if let Err(e) = &result {
        // A cancelled download is not a failure, and the UI must not shout about one. Partial
        // files stay in staging so the next attempt resumes instead of restarting.
        if cancel.is_cancelled() {
            let _ = channel.send(json!({ "event": "cancelled" }));
            return Ok(());
        }
        let _ = channel.send(json!({ "event": "failed", "message": e }));
    }
    result
}

async fn install_inner(
    id: &str,
    root: PathBuf,
    cancel: CancellationToken,
    channel: &Channel<Value>,
) -> Result<(), String> {
    let catalog = Catalog::builtin().map_err(|e| e.to_string())?;
    let entry = catalog
        .get(id)
        .ok_or_else(|| format!("unknown model id '{id}'"))?;
    let manifest_name = entry
        .manifest
        .as_ref()
        .ok_or_else(|| format!("'{id}' has no pinned manifest, so it cannot be downloaded safely"))?;
    let mdir = manifests_dir(None).ok_or_else(|| "manifest directory not found".to_string())?;
    let path = mdir.join(manifest_name);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest: ModelManifest =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    manifest.validate().map_err(|e| e.to_string())?;

    let (target, files) = manifest
        .select_files(&preferred_targets(probe_capabilities()))
        .ok_or_else(|| format!("no artifact in '{manifest_name}' matches this machine"))?;
    tracing::info!("installing {id} ({} files, target {target:?})", files.len());

    let registry = ModelRegistry::new(&root);
    let final_dir = registry.model_dir(&manifest);
    let staging = staging_dir_for(&final_dir);

    let downloader = ModelDownloader::new().map_err(|e| e.to_string())?;
    let ch = channel.clone();
    // `Progress` carries indices, not a name, so remember the file the last `FileStarted`
    // announced and report progress against it.
    let mut current = String::new();
    downloader
        .install(&files, &staging, &final_dir, cancel, move |ev| {
            let payload = match ev {
                DownloadEvent::FileStarted { path, total } => {
                    current = path.clone();
                    json!({ "event": "file_started", "path": path, "total": total })
                }
                DownloadEvent::Progress(p) => json!({
                    "event": "progress",
                    "path": current,
                    "received": p.file_downloaded,
                    "total": p.file_total,
                    "file_index": p.file_index,
                    "file_count": p.file_count,
                }),
                DownloadEvent::FileVerified { path } => {
                    json!({ "event": "file_verified", "path": path })
                }
                DownloadEvent::Completed => json!({ "event": "completed" }),
            };
            let _ = ch.send(payload);
        })
        .await
        .map_err(|e| e.to_string())?;

    let _ = channel.send(json!({
        "event": "completed",
        "dir": final_dir.display().to_string(),
    }));
    Ok(())
}

/// Ask an in-flight [`install_model`] to stop. Partial files remain in staging and resume later.
#[tauri::command]
pub fn cancel_install(state: State<'_, AppState>, id: String) {
    if let Some(token) = state.installs.lock().get(&id) {
        token.cancel();
    }
}

/// What the dictation engine is running on right now.
///
/// The answer comes from the engine itself, not from settings: a run that asked for the NPU and
/// fell back reports the CPU, and `notes` says why. Before the first dictation the engine has not
/// been loaded and `loaded` is false — the UI should say so rather than predict.
#[tauri::command]
pub async fn active_backend(state: State<'_, AppState>) -> Result<crate::worker::ActiveBackend, String> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    state.worker.send(WorkerCmd::Describe { reply: tx });
    tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(std::time::Duration::from_secs(20))
            .map_err(|_| "the dictation worker did not answer".to_string())
    })
    .await
    .map_err(|e| format!("active_backend task failed: {e}"))?
}

/// Open or close a microphone stream that only drives the level meter.
///
/// Nothing is transcribed and nothing is kept: the audio is read for its amplitude and dropped.
/// It exists so "is my microphone working?" can be answered on the first screen without
/// dictating into something, and so the meter shows a real signal rather than an animation.
///
/// Returns whether the stream is open afterwards.
#[tauri::command]
pub async fn set_mic_test(state: State<'_, AppState>, enabled: bool) -> Result<bool, String> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    state.worker.send(WorkerCmd::MicTest { enabled, reply: tx });
    tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|_| Err("the dictation worker did not answer".to_string()))
    })
    .await
    .map_err(|e| format!("microphone test task failed: {e}"))?
}

/// Measure a model on **every usable accelerator** and return the comparison.
///
/// One action instead of "pick a backend, run, write the number down, repeat". All runs use the
/// same clips, which is what makes the numbers comparable at all. Long: a first NPU run can spend
/// minutes preparing its context binary, so the backend emits `benchmark_progress` as it goes.
#[tauri::command]
pub async fn run_benchmark_all(
    state: State<'_, AppState>,
    model_id: Option<String>,
) -> Result<crate::worker::BenchSuite, String> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let settings_path = state.settings_path.clone();
    state.worker.send(WorkerCmd::BenchmarkAll { model_id, reply: tx });
    let suite = tauri::async_runtime::spawn_blocking(move || {
        rx.recv()
            .unwrap_or_else(|_| Err("benchmark worker stopped".to_string()))
    })
    .await
    .map_err(|e| format!("benchmark task failed: {e}"))?;

    // A sweep measures one model on several accelerators, so it fills several records at once --
    // which is the case the "on your machine" column is most useful for.
    if let Ok(s) = &suite {
        for run in &s.runs {
            record_local_measurement(&settings_path, run);
        }
    }
    suite
}

/// Measure a model on this machine.
///
/// Runs on the dictation worker thread so that no second engine can exist while the dictation
/// engine is loaded -- two QNN sessions would contend for the same Hexagon context.
#[tauri::command]
pub async fn run_benchmark(
    state: State<'_, AppState>,
    model_id: Option<String>,
    backend: Option<BackendPreference>,
) -> Result<BenchReport, String> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    // Copied before the await: `State` is not held across it.
    let settings_path = state.settings_path.clone();
    state.worker.send(WorkerCmd::Benchmark {
        model_id,
        backend,
        reply: tx,
    });
    let report = tauri::async_runtime::spawn_blocking(move || {
        rx.recv()
            .unwrap_or_else(|_| Err("benchmark worker stopped".to_string()))
    })
    .await
    .map_err(|e| format!("benchmark task failed: {e}"))?;

    // Keep what this machine measured, beside the model it measured. Recorded here rather than in
    // the frontend so a run started from anywhere is kept, and so closing the window does not lose
    // a measurement that took minutes to produce.
    if let Ok(r) = &report {
        record_local_measurement(&settings_path, r);
    }
    report
}

/// Persist one benchmark result. Never fatal: a benchmark that ran is still a benchmark the user
/// can read on screen, and failing the command because a cache file could not be written would
/// throw away the thing they waited for.
fn record_local_measurement(settings_path: &std::path::Path, report: &BenchReport) {
    let Some(entry) = lw_app::measurements::from_report(report) else {
        return;
    };
    let path = lw_app::measurements::path_for(settings_path);
    let mut set = lw_app::measurements::LocalMeasurements::load(&path);
    set.record(&report.model_id, entry);
    if let Err(e) = set.save(&path) {
        tracing::warn!("could not save local measurements: {e}");
    }
}

/// Delete an installed model's files, freeing the disk space.
///
/// Destructive and irreversible, so the path is never taken from the caller: the frontend sends a
/// catalog **id**, this resolves the directory through the same manifest the installer used, and
/// then checks the result really does sit inside the models root before removing anything. A
/// frontend bug, or a call from anywhere else, cannot turn into "delete this directory".
///
/// Refuses to delete the model dictation is currently set to use. The confirmation dialog guards
/// against a mis-click; this guards against a choice whose consequence appears later, at the worst
/// moment — the next time the hotkey is pressed, with no model to load.
#[tauri::command]
pub async fn delete_model(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    let root = models_root(&state);
    let settings = lw_core::settings::Settings::load(&state.settings_path).map_err(|e| e.to_string())?;
    if settings.model_id == id {
        return Err(format!(
            "'{id}' is the model dictation is set to use. Choose another model first, then delete this one."
        ));
    }

    let catalog = Catalog::builtin().map_err(|e| e.to_string())?;
    let entry = catalog
        .get(&id)
        .ok_or_else(|| format!("unknown model id '{id}'"))?;
    let manifest_name = entry
        .manifest
        .as_ref()
        .ok_or_else(|| format!("'{id}' has no manifest, so nothing was installed for it"))?;
    let mdir = manifests_dir(None).ok_or_else(|| "manifest directory not found".to_string())?;
    let text = std::fs::read_to_string(mdir.join(manifest_name)).map_err(|e| e.to_string())?;
    let manifest: ModelManifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    manifest.validate().map_err(|e| e.to_string())?;

    let dir = ModelRegistry::new(&root).model_dir(&manifest);
    let freed = tauri::async_runtime::spawn_blocking(move || remove_model_dir(&root, &dir))
        .await
        .map_err(|e| format!("delete task failed: {e}"))??;

    Ok(json!({ "id": id, "freed_bytes": freed }))
}

/// Remove `dir`, having proved it is inside `root`. Returns the bytes freed.
fn remove_model_dir(root: &std::path::Path, dir: &std::path::Path) -> Result<u64, String> {
    if !dir.exists() {
        return Ok(0);
    }
    // Compare canonical paths: `..` in either would otherwise let a directory outside the models
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
fn dir_size(dir: &std::path::Path) -> u64 {
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

/// Everything this machine has measured, for the model list.
#[tauri::command]
pub fn local_measurements(state: State<'_, AppState>) -> Value {
    let path = lw_app::measurements::path_for(&state.settings_path);
    serde_json::to_value(lw_app::measurements::LocalMeasurements::load(&path).models)
        .unwrap_or_else(|_| json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_json_separates_present_from_usable() {
        let mut caps = Capabilities::unknown();
        caps.npu.present = true;
        caps.npu.htp_arch = Some(lw_core::capabilities::HtpArch::V81);
        caps.providers.qnn = false;

        let v = capabilities_json(&caps);
        assert_eq!(v["npu_present"], json!(true));
        assert_eq!(v["npu_usable"], json!(false));
        assert_eq!(v["npu_label"], json!("Hexagon V81"));

        caps.providers.qnn = true;
        assert_eq!(capabilities_json(&caps)["npu_usable"], json!(true));
    }

    #[test]
    fn capabilities_json_has_every_field_the_frontend_reads() {
        let v = capabilities_json(&Capabilities::unknown());
        for key in [
            "machine",
            "cpu",
            "cores",
            "os",
            "arch",
            "npu_present",
            "npu_label",
            "npu_usable",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
    }

    /// The guard that matters: a directory outside the models root is never removed, whatever the
    /// manifest says, because `remove_dir_all` on the wrong path is not something a confirmation
    /// dialog can take back.
    #[test]
    fn deleting_refuses_a_directory_outside_the_models_root() {
        let base = std::env::temp_dir().join("lw-delete-guard");
        let root = base.join("models");
        let outside = base.join("not-models");
        std::fs::create_dir_all(root.join("keep")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("precious.txt"), b"x").unwrap();

        let err = remove_model_dir(&root, &outside).unwrap_err();
        assert!(err.contains("refusing to delete"), "{err}");
        assert!(
            outside.join("precious.txt").exists(),
            "nothing outside may be touched"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The models root itself is not a model directory.
    #[test]
    fn deleting_refuses_the_models_root_itself() {
        let root = std::env::temp_dir().join("lw-delete-root");
        std::fs::create_dir_all(&root).unwrap();
        let err = remove_model_dir(&root, &root).unwrap_err();
        assert!(err.contains("refusing to delete"), "{err}");
        assert!(root.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deleting_a_model_directory_removes_it_and_reports_the_space() {
        let root = std::env::temp_dir().join("lw-delete-ok");
        let model = root.join("some-model");
        std::fs::create_dir_all(model.join("nested")).unwrap();
        std::fs::write(model.join("a.onnx"), vec![0u8; 100]).unwrap();
        std::fs::write(model.join("nested/b.txt"), vec![0u8; 23]).unwrap();

        let freed = remove_model_dir(&root, &model).unwrap();
        assert_eq!(freed, 123, "the whole tree is counted");
        assert!(!model.exists());
        assert!(root.exists(), "only the model directory goes");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Deleting something already gone is success, not an error: the end state is what was asked.
    #[test]
    fn deleting_a_missing_directory_is_not_an_error() {
        let root = std::env::temp_dir().join("lw-delete-missing");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(remove_model_dir(&root, &root.join("nope")).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn engine_features_always_include_parakeet() {
        assert!(lw_app::catalog::engine_features().contains(&"parakeet"));
    }

    /// The shipped Windows build turns the `sherpa` feature on (see
    /// `scripts/build/build-windows-arm64.ps1`), because fourteen of the fifteen catalog entries
    /// are gated on it and installing one of those models in a build without it produces a model
    /// that downloads, verifies, and then cannot be loaded. This pins the whole chain: the feature
    /// reaches the engine crate, the engine crate reports itself available, and the catalog then
    /// marks a gated entry runnable rather than blocked.
    #[test]
    #[cfg(feature = "sherpa")]
    fn a_sherpa_build_can_actually_run_the_gated_models() {
        assert!(
            lw_app::catalog::engine_features().contains(&"sherpa"),
            "the feature is on for this crate but the engine crate says it is not linked"
        );

        let catalog = Catalog::builtin().expect("builtin catalog");
        let gated = catalog
            .entries
            .iter()
            .find(|e| e.requires_engine_feature.as_deref() == Some("sherpa"))
            .expect("the catalog is supposed to contain sherpa-gated entries");

        let recs = catalog.recommend_with(probe_capabilities(), &lw_app::catalog::engine_features());
        let rec = recs
            .iter()
            .find(|r| r.entry.id == gated.id)
            .expect("every entry is ranked");
        assert!(
            !rec.blockers.iter().any(|b| b.contains("sherpa")),
            "{} still reports a sherpa blocker in a sherpa build: {:?}",
            gated.id,
            rec.blockers
        );
    }

    #[test]
    fn catalog_json_shape_matches_the_frontend_contract() {
        let root = std::env::temp_dir().join("lw-catalog-json-test");
        let v = build_catalog_json(root).expect("builtin catalog must build");
        for key in [
            "machine",
            "models_root",
            "manifests_dir",
            "recommended",
            "estimate_disclaimer",
            "entries",
        ] {
            assert!(v.get(key).is_some(), "missing top-level {key}");
        }
        let entries = v["entries"].as_array().expect("entries is an array");
        assert!(!entries.is_empty(), "builtin catalog must not be empty");
        for key in [
            "id",
            "name",
            "description",
            "engine",
            "licence",
            "languages",
            "language_summary",
            "quality",
            "quality_label",
            "speed",
            "speed_label",
            "download_bytes",
            "hardware",
            "runnable",
            "estimated_rtf",
            "measured_reference",
            "wer_estimates",
            "reason",
            "blockers",
            "install_state",
        ] {
            assert!(entries[0].get(key).is_some(), "entry missing {key}");
        }
    }

    #[test]
    fn estimated_and_measured_are_separate_fields() {
        // The UI can only label them differently if the backend keeps them apart.
        let root = std::env::temp_dir().join("lw-catalog-json-test2");
        let v = build_catalog_json(root).unwrap();
        let e = &v["entries"][0];
        assert_ne!(
            e.get("estimated_rtf"),
            e.get("measured_reference"),
            "an estimate must never be served as a measurement"
        );
    }
}
