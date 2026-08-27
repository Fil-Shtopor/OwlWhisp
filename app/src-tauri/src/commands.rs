//! Tauri IPC commands exposed to the frontend.
//!
//! Contract (all snake_case):
//! - `get_settings() -> Settings JSON` / `set_settings(settings) -> Settings JSON`
//! - `list_backends() -> ["automatic", "force_npu", "force_cpu"]`
//! - `get_diagnostics() -> JSON object` (ORT runtime discovery + QNN/NPU status)
//! - `get_recording_state() -> "idle" | ...` / `set_recording_state(next)`
//! - `subscribe_mic_level(channel)` — registers a `Channel<f32>` for mic-level frames.

use lw_core::settings::Settings;
use serde_json::{Value, json};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};

use crate::state::{AppState, RecordingState};

/// Load settings from `app_data_dir/settings.json` (defaults if the file is missing).
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Value, String> {
    let settings = Settings::load(&state.settings_path).map_err(|e| e.to_string())?;
    serde_json::to_value(&settings).map_err(|e| e.to_string())
}

/// Validate and persist settings atomically via `lw_core::settings::Settings`.
/// Returns the normalized settings as saved.
#[tauri::command]
pub fn set_settings(app: AppHandle, state: State<'_, AppState>, settings: Value) -> Result<Value, String> {
    let parsed: Settings = serde_json::from_value(settings).map_err(|e| e.to_string())?;
    parsed.save(&state.settings_path).map_err(|e| e.to_string())?;
    state
        .overlay_enabled
        .store(parsed.overlay_enabled, std::sync::atomic::Ordering::Relaxed);
    // Apply the parts that live outside the settings file: rebind the global hotkey (and its
    // hold/toggle behaviour) and make the worker pick up the new model/backend/dictionary.
    crate::reregister_shortcut(&app, &parsed);
    state.worker.send(crate::worker::WorkerCmd::ReloadSettings);
    serde_json::to_value(&parsed).map_err(|e| e.to_string())
}

/// The accelerator currently registered with the OS, for the shortcut editor to display.
#[tauri::command]
pub fn active_hotkey(state: State<'_, AppState>) -> Option<String> {
    state.shortcut().map(|s| s.into_string())
}

/// The backend preferences the UI can offer (serde names of `lw_core`'s `BackendPreference`).
#[tauri::command]
pub fn list_backends() -> Vec<String> {
    use lw_core::engine::BackendPreference;
    [
        BackendPreference::Automatic,
        BackendPreference::ForceNpu,
        BackendPreference::ForceCpu,
    ]
    .iter()
    .map(|p| {
        serde_json::to_value(p)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    })
    .collect()
}

/// Current recording state (lowercase string).
#[tauri::command]
pub fn get_recording_state(state: State<'_, AppState>) -> RecordingState {
    state.recording_state()
}

/// Force a recording state (used by the UI for testing; the hotkey drives it normally).
#[tauri::command]
pub fn set_recording_state(app: AppHandle, state: State<'_, AppState>, next: RecordingState) {
    state.transition(&app, next);
}

/// Register the mic-level stream. The backend pushes `f32` levels in `[0, 1]` while listening.
///
/// TODO: wire to lw-platform AudioCapture — today the levels are synthesized by
/// `spawn_mic_level_stub` in `lib.rs` purely so the UI meter can be developed against
/// a real `tauri::ipc::Channel`.
#[tauri::command]
pub fn subscribe_mic_level(state: State<'_, AppState>, channel: Channel<f32>) {
    *state.mic_level.lock() = Some(channel);
}

/// Structured diagnostics: app/OS info plus ONNX Runtime discovery and QNN/NPU status.
/// Runtime probing loads onnxruntime.dll, so it runs on a blocking thread.
#[tauri::command]
pub async fn get_diagnostics() -> Value {
    tauri::async_runtime::spawn_blocking(collect_diagnostics)
        .await
        .unwrap_or_else(|e| json!({ "error": format!("diagnostics task failed: {e}") }))
}

fn collect_diagnostics() -> Value {
    let mut m = serde_json::Map::new();
    m.insert("app_version".into(), json!(env!("CARGO_PKG_VERSION")));
    m.insert("core_version".into(), json!(lw_core::VERSION));
    m.insert("os".into(), json!(std::env::consts::OS));
    m.insert("os_version".into(), json!(tauri_plugin_os::version().to_string()));
    m.insert("arch".into(), json!(std::env::consts::ARCH));

    match lw_ort::OrtRuntime::auto() {
        Ok(rt) => {
            m.insert(
                "runtime_dir".into(),
                json!(rt.runtime_dir().display().to_string()),
            );
            m.insert("qnn_dll_present".into(), json!(rt.qnn_available()));
            // NOTE: has_qnn_npu() lazily registers the QNN plugin EP process-wide. That is fine
            // while this process runs no inference, but once engines are wired in, CPU sessions
            // must be built *before* the first NPU probe (see lw-ort's OrtRuntime docs).
            let npu_available = rt.has_qnn_npu();
            m.insert("npu_available".into(), json!(npu_available));
            m.insert("qnn_registered".into(), json!(rt.qnn_registered()));
            m.insert("qnn_npu_count".into(), json!(rt.qnn_npu_count()));
            m.insert("devices".into(), json!(rt.device_summary()));
        }
        Err(e) => {
            m.insert("runtime_error".into(), json!(e.to_string()));
            m.insert("qnn_dll_present".into(), json!(false));
            m.insert("npu_available".into(), json!(false));
        }
    }

    Value::Object(m)
}
