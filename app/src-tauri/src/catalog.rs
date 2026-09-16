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
use std::sync::OnceLock;

use lw_core::capabilities::Capabilities;
use lw_core::engine::BackendPreference;
use lw_core::model::{
    CancellationToken, Catalog, DownloadEvent, ModelDownloader, ModelManifest, ModelRegistry,
    default_models_root, entry_paths, manifests_dir, preferred_targets, staging_dir_for,
};
use serde_json::{Value, json};
use tauri::State;
use tauri::ipc::Channel;

use crate::state::AppState;
use crate::worker::{BenchReport, WorkerCmd};

/// The sentence the UI must show beside any estimated number.
const ESTIMATE_DISCLAIMER: &str = "Estimated from the model's speed tier and your detected hardware - not a measurement. \
     Run the benchmark for a real number on this machine.";

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
/// Enumerating devices is the only honest way to answer "is the NPU usable", so that is what this
/// does. It registers the QNN plugin EP process-wide, which is safe here because every CPU session
/// is pinned to the CPU device (see `lw_ort::build_cpu_session`). The result is cached: the probe
/// costs a DLL load, and the answer cannot change while the process runs.
pub fn probe_capabilities() -> &'static Capabilities {
    static CAPS: OnceLock<Capabilities> = OnceLock::new();
    CAPS.get_or_init(|| {
        let mut caps = lw_platform::caps::detect();
        caps.providers.cpu = true;
        match lw_ort::OrtRuntime::auto() {
            Ok(rt) => caps.providers.qnn = rt.has_qnn_npu(),
            Err(e) => tracing::warn!(
                "ONNX Runtime not loaded ({e}); accelerator availability is unverified,                  so everything reported here assumes CPU"
            ),
        }
        caps
    })
}

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

/// Engine features compiled into *this* build, which decide whether an entry is runnable here.
fn engine_features() -> Vec<&'static str> {
    let mut f = vec!["parakeet"];
    // Ask the engine crate rather than testing a feature flag on this crate: it knows whether its
    // native library is linked, and this keeps the app and the CLI from disagreeing.
    if lw_engine_sherpa::is_available() {
        f.push("sherpa");
    }
    f
}

/// The catalog, matched against this machine and this disk.
#[tauri::command]
pub async fn list_models(state: State<'_, AppState>) -> Result<Value, String> {
    let root = models_root(&state);
    tauri::async_runtime::spawn_blocking(move || build_catalog_json(root))
        .await
        .map_err(|e| format!("catalog task failed: {e}"))?
}

fn build_catalog_json(root: PathBuf) -> Result<Value, String> {
    let catalog = Catalog::builtin().map_err(|e| e.to_string())?;
    let caps = probe_capabilities();
    let manifests = manifests_dir(None);
    let features = engine_features();
    let recs = catalog.recommend_with(caps, &features);

    // `recommend_with` sorts best-first, so the first runnable entry is the recommendation.
    let recommended = recs.iter().find(|r| r.runnable).map(|r| r.entry.id.clone());

    let entries: Vec<Value> = recs
        .iter()
        .map(|r| {
            let e = r.entry;
            let paths = entry_paths(e, manifests.as_deref(), &root, caps);
            json!({
                "id": e.id,
                "name": e.name,
                "description": e.description,
                "engine": e.engine.as_str(),
                "licence": e.license,
                "upstream_url": e.source_url,
                "languages": e.languages,
                "language_summary": e.language_summary(),
                "quality": e.quality,
                "quality_label": e.quality.label(),
                "speed": e.speed,
                "speed_label": e.speed.label(),
                "download_bytes": e.download_bytes,
                "disk_bytes": e.disk_bytes,
                "hardware": e.hardware.iter().map(|h| h.label()).collect::<Vec<_>>(),
                "runnable": r.runnable,
                "best_hardware": r.best_hardware.map(|h| h.label()),
                "estimated_rtf": r.estimated_rtf,
                "measured_reference": r.measured_reference,
                "measurements": e.measurements,
                "wer_estimates": e.wer_estimates,
                "reason": r.reason,
                "blockers": r.blockers,
                "install_state": paths.state,
                "install_dir": paths.dir.map(|d| d.display().to_string()),
                "manifest_error": paths.manifest_error,
                "notes": (!e.notes.is_empty()).then(|| e.notes.clone()),
            })
        })
        .collect();

    Ok(json!({
        "machine": caps.summary(),
        "models_root": root.display().to_string(),
        "manifests_dir": manifests.map(|d| d.display().to_string()),
        "recommended": recommended,
        "estimate_disclaimer": ESTIMATE_DISCLAIMER,
        "entries": entries,
    }))
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
    state.worker.send(WorkerCmd::BenchmarkAll { model_id, reply: tx });
    tauri::async_runtime::spawn_blocking(move || {
        rx.recv()
            .unwrap_or_else(|_| Err("benchmark worker stopped".to_string()))
    })
    .await
    .map_err(|e| format!("benchmark task failed: {e}"))?
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
    state.worker.send(WorkerCmd::Benchmark {
        model_id,
        backend,
        reply: tx,
    });
    tauri::async_runtime::spawn_blocking(move || {
        rx.recv()
            .unwrap_or_else(|_| Err("benchmark worker stopped".to_string()))
    })
    .await
    .map_err(|e| format!("benchmark task failed: {e}"))?
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

    #[test]
    fn engine_features_always_include_parakeet() {
        assert!(engine_features().contains(&"parakeet"));
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
