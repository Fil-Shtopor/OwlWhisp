//! The dictation worker: owns the ONNX Runtime, the Parakeet engine and the microphone capture on
//! a dedicated background thread, and turns hotkey press/release into captured-audio →
//! transcription → text pipeline → injection.
//!
//! The engine holds an ORT `Session` (Send but not Sync), so it cannot live in Tauri's shared
//! `State`. Instead the worker runs on its own thread and the app talks to it over a channel; the
//! app only holds the `Sender`, which is `Send + Sync`.
//!
//! Model/runtime discovery:
//! - runtime dir: `LW_RUNTIME_DIR`, else `<exe dir>/runtime/win-arm64`, else `<exe dir>`.
//! - model dir: `<app_data_dir>/models/<model_id>` (from settings).
//!
//! If the runtime or model is missing, the worker reports an `Error` state instead of crashing; the
//! rest of the app (tray, settings, diagnostics) keeps working.

use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender};
use lw_core::audio::AudioBuffer;
use lw_core::dictionary::Dictionary;
use lw_core::engine::{BackendPreference, EngineInitContext, SpeechEngine};
use lw_core::settings::Settings;
use lw_core::text::{CleanupProcessor, DictionaryProcessor, NormalizeOptions, TextPipeline, TextProcessor};
use lw_engine_parakeet::{BackendKind, ParakeetConfig, ParakeetEngine};
use lw_platform::{AudioCapture, Capture};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{AppState, RecordingState};

/// Commands sent to the worker thread.
pub enum WorkerCmd {
    /// Begin capturing from the microphone.
    StartRecording,
    /// Stop capturing and transcribe → deliver the resulting text.
    StopRecording,
    /// Reload settings (backend, dictionary, cleanup) for the next utterance.
    ReloadSettings,
    /// Shut the worker down.
    Shutdown,
}

/// Emitted to the frontend as `transcript` when an utterance is delivered.
#[derive(Clone, serde::Serialize)]
pub struct TranscriptPayload {
    /// The final (cleaned) text.
    pub text: String,
    /// Whether it was injected into the foreground app (vs. copied to the clipboard).
    pub injected: bool,
    /// Backend that produced it, e.g. "QNN" / "ONNX Runtime CPU".
    pub provider: String,
}

/// Handle held by the app; forwards commands to the worker thread.
#[derive(Clone)]
pub struct WorkerHandle {
    tx: Sender<WorkerCmd>,
}

impl WorkerHandle {
    /// Send a command (ignored if the worker has stopped).
    pub fn send(&self, cmd: WorkerCmd) {
        let _ = self.tx.send(cmd);
    }
}

/// Spawn the worker thread. `app` is used to emit events; `settings_path` locates settings and,
/// via its parent, the app-data model directory.
pub fn spawn(app: AppHandle, settings_path: PathBuf) -> WorkerHandle {
    let (tx, rx) = crossbeam_channel::unbounded::<WorkerCmd>();
    std::thread::Builder::new()
        .name("dictation-worker".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || worker_loop(app, settings_path, rx))
        .expect("spawn dictation worker");
    WorkerHandle { tx }
}

fn app_data_dir(settings_path: &std::path::Path) -> PathBuf {
    settings_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default()
}

fn runtime_dir() -> Option<PathBuf> {
    // `LW_RUNTIME_DIR`, then `runtime/<platform>/` and `runtime/` beside the executable.
    lw_ort::locate_runtime_dir()
}

struct Loaded {
    engine: ParakeetEngine,
    settings: Settings,
}

fn load_engine(settings_path: &std::path::Path) -> Result<Loaded, String> {
    let settings = Settings::load(settings_path).map_err(|e| e.to_string())?;
    let rt_dir = runtime_dir().ok_or_else(|| "ONNX Runtime not found (set LW_RUNTIME_DIR)".to_string())?;
    let runtime = lw_ort::OrtRuntime::init(&rt_dir).map_err(|e| e.to_string())?;

    let data = app_data_dir(settings_path);
    let model_dir = data.join("models").join(&settings.model_id);
    let cache_dir = data.join("cache");

    let backend = match settings.backend {
        BackendPreference::Automatic => BackendKind::Auto,
        BackendPreference::ForceNpu => BackendKind::ForceNpu,
        BackendPreference::ForceCpu => BackendKind::ForceCpu,
    };
    let mut config = ParakeetConfig::from_ctx(
        &EngineInitContext {
            model_dir: model_dir.clone(),
            cache_dir: cache_dir.clone(),
            cpu_threads: 0,
        },
        backend,
    );
    if runtime.qnn_available() {
        config.htp_arch = Some(81);
        config.soc_model = Some(88);
    }
    let mut engine = ParakeetEngine::new(runtime, config);
    engine
        .initialize(&EngineInitContext {
            model_dir,
            cache_dir,
            cpu_threads: 0,
        })
        .map_err(|e| e.to_string())?;
    Ok(Loaded { engine, settings })
}

fn build_pipeline(settings: &Settings) -> TextPipeline {
    let mut pipeline = TextPipeline::new();
    // Deterministic cleanup (whitespace + NFC always; casing/punctuation are conservative defaults).
    pipeline.push(Box::new(CleanupProcessor::new(NormalizeOptions::default())) as Box<dyn TextProcessor>);
    if !settings.dictionary.rules.is_empty() {
        let dict: Dictionary = settings.dictionary.clone();
        pipeline.push(Box::new(DictionaryProcessor::new(dict)));
    }
    pipeline
}

fn worker_loop(app: AppHandle, settings_path: PathBuf, rx: Receiver<WorkerCmd>) {
    // Lazy-load the engine on first use so app startup is not blocked by the 650 MB model.
    let mut loaded: Option<Result<Loaded, String>> = None;
    let mut capture: Option<Capture> = None;

    let emit_error = |app: &AppHandle, msg: &str| {
        tracing::error!("dictation worker: {msg}");
        let state = app.state::<AppState>();
        state.transition(app, RecordingState::Error);
        let _ = app.emit("worker_error", msg);
        state.transition(app, RecordingState::Idle);
    };

    while let Ok(cmd) = rx.recv() {
        match cmd {
            WorkerCmd::Shutdown => break,
            WorkerCmd::ReloadSettings => {
                loaded = None; // reload lazily next utterance
            }
            WorkerCmd::StartRecording => {
                let mut cap = Capture::new(None, 16_000 * 120);
                match cap.start() {
                    Ok(()) => capture = Some(cap),
                    Err(e) => emit_error(&app, &format!("microphone: {e}")),
                }
            }
            WorkerCmd::StopRecording => {
                let Some(mut cap) = capture.take() else { continue };
                let audio: AudioBuffer = match cap.stop() {
                    Ok(a) => a,
                    Err(e) => {
                        emit_error(&app, &format!("capture stop: {e}"));
                        continue;
                    }
                };
                // Load the engine on first use.
                if loaded.is_none() {
                    loaded = Some(load_engine(&settings_path));
                }
                let Some(Ok(state)) = loaded.as_mut() else {
                    let msg = loaded
                        .as_ref()
                        .and_then(|r| r.as_ref().err())
                        .cloned()
                        .unwrap_or_else(|| "engine unavailable".into());
                    emit_error(&app, &msg);
                    continue;
                };
                let provider = state.engine.provider().to_string();
                let transcript = match state.engine.transcribe(&audio) {
                    Ok(t) => t,
                    Err(e) => {
                        emit_error(&app, &format!("transcribe: {e}"));
                        continue;
                    }
                };
                let pipeline = build_pipeline(&state.settings);
                let text = pipeline.run(&transcript.text);

                // Deliver: inject into the foreground app, else fall back to clipboard.
                let injected = deliver(&text);
                let _ = app.emit(
                    "transcript",
                    TranscriptPayload {
                        text,
                        injected,
                        provider,
                    },
                );
                let app_state = app.state::<AppState>();
                app_state.transition(&app, RecordingState::Done);
                app_state.transition(&app, RecordingState::Idle);
            }
        }
    }
    let _ = &app;
}

/// Inject `text` into the foreground application; on failure, copy to the clipboard. Returns whether
/// it was injected (vs. copied). Empty text is a no-op.
fn deliver(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    let platform = lw_platform::platform();
    if let Ok(mut injector) = platform.injector()
        && injector.inject(text).is_ok()
    {
        return true;
    }
    if let Ok(mut clip) = platform.clipboard() {
        let _ = clip.set_text(text);
    }
    false
}
