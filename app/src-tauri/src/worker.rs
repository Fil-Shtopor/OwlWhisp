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
    StartRecording {
        /// End the utterance automatically once the speaker stops (hands-free mode) instead of
        /// waiting for a second key press.
        hands_free: bool,
    },
    /// Stop capturing and transcribe → deliver the resulting text.
    StopRecording,
    /// Reload settings (backend, dictionary, cleanup) for the next utterance.
    ReloadSettings,
    /// Report what the loaded engine is actually running on, without loading one.
    Describe {
        /// Where to send the description.
        reply: Sender<ActiveBackend>,
    },
    /// Measure a model on this machine and reply with the report.
    ///
    /// Benchmarks run **on the worker thread** rather than a fresh one so that a second engine
    /// can never exist alongside the dictation engine: two QNN sessions would compete for the
    /// same Hexagon context, and the failure would look like a benchmark result.
    Benchmark {
        /// Model to measure; `None` means whatever Settings currently selects.
        model_id: Option<String>,
        /// Backend to force; `None` means the configured preference.
        backend: Option<BackendPreference>,
        /// Where to send the report.
        reply: Sender<Result<BenchReport, String>>,
    },
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
    engine: Box<dyn SpeechEngine>,
    settings: Settings,
}

fn load_engine(settings_path: &std::path::Path) -> Result<Loaded, String> {
    let settings = Settings::load(settings_path).map_err(|e| e.to_string())?;
    let data = app_data_dir(settings_path);
    let model_dir = data.join("models").join(&settings.model_id);
    let cache_dir = data.join("cache");
    if !model_dir.exists() {
        return Err(format!(
            "model '{}' is not installed ({})",
            settings.model_id,
            model_dir.display()
        ));
    }
    let ctx = EngineInitContext {
        model_dir: model_dir.clone(),
        cache_dir: cache_dir.clone(),
        cpu_threads: 0,
    };

    // Pick the engine from what is actually in the model directory, so switching models in
    // Settings is enough to switch engines — no per-model wiring in the UI.
    let mut engine = build_engine_for(&model_dir, &settings, &ctx)?;
    engine.initialize(&ctx).map_err(|e| e.to_string())?;
    Ok(Loaded { engine, settings })
}

/// Choose an engine for the model directory's contents.
///
/// A sherpa-onnx style layout (Whisper / Moonshine / SenseVoice / NeMo transducer) goes to
/// [`lw_engine_sherpa`]; anything else is treated as the Parakeet TDT layout, which is the only one
/// that can use the Qualcomm NPU.
fn build_engine_for(
    model_dir: &std::path::Path,
    settings: &Settings,
    ctx: &EngineInitContext,
) -> Result<Box<dyn SpeechEngine>, String> {
    if let Ok(files) = lw_engine_sherpa::detect_in_dir(model_dir, true, None) {
        let kind = files.kind();
        #[cfg(feature = "sherpa")]
        {
            tracing::info!(
                "model '{}' detected as {kind}; using the sherpa engine",
                settings.model_id
            );
            let cfg = lw_engine_sherpa::SherpaConfig::new(model_dir);
            return Ok(Box::new(lw_engine_sherpa::SherpaEngine::new(cfg)));
        }
        #[cfg(not(feature = "sherpa"))]
        {
            return Err(format!(
                "model '{}' is a {kind} model, which needs a build with the `sherpa` engine feature",
                settings.model_id
            ));
        }
    }

    let rt_dir = runtime_dir().ok_or_else(|| "ONNX Runtime not found (set LW_RUNTIME_DIR)".to_string())?;
    let runtime = lw_ort::OrtRuntime::init(&rt_dir).map_err(|e| e.to_string())?;
    let backend = BackendKind::from(settings.backend);
    // Hexagon generation comes from the detected NPU (V73 on X Elite / X Plus, V81 on X2 Elite),
    // never from a hard-coded assumption about one SoC.
    let config =
        ParakeetConfig::from_ctx(ctx, backend).with_capabilities(crate::catalog::probe_capabilities());
    Ok(Box::new(ParakeetEngine::new(runtime, config)))
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

/// What the dictation engine is running on right now.
///
/// `loaded` is false until the first dictation, because the engine loads lazily — and the honest
/// answer before that is "nothing yet", not a prediction from settings.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ActiveBackend {
    /// Whether an engine is loaded at all.
    pub loaded: bool,
    /// The execution provider that actually ran, read back from the engine.
    pub provider: Option<String>,
    /// Its acceleration class (CPU / GPU / NPU / ANE), for display.
    pub acceleration: Option<String>,
    /// The selected accelerator's stable id (`"qnn_npu"`, `"web_gpu"`, …). Compare against this,
    /// never against the display strings above.
    pub accelerator: Option<String>,
    /// That accelerator's class as a stable id (`"cpu"` / `"gpu"` / `"npu"`).
    pub accelerator_kind: Option<String>,
    /// Device description.
    pub device: Option<String>,
    /// Backend-selection notes, including the reason for any fallback.
    pub notes: Vec<String>,
    /// The model it loaded.
    pub model_id: String,
    /// Why loading failed, when it did.
    pub error: Option<String>,
}

/// A benchmark report: every number in it was measured on this machine by this run.
#[derive(Clone, Debug, serde::Serialize)]
pub struct BenchReport {
    /// One-line description of the machine.
    pub machine: String,
    /// The backend that **actually** executed, read back from the engine rather than requested.
    pub backend: String,
    /// The model that was measured.
    pub model_id: String,
    /// Where its files were loaded from.
    pub model_dir: String,
    /// Where the audio came from, including whether WER was computable.
    pub clip_source: String,
    /// Time spent constructing and initializing the engine.
    pub engine_load_ms: f32,
    /// Per-clip results in run order.
    pub clips: Vec<lw_core::bench::ClipResult>,
    /// First run, which includes one-time warm-up.
    pub cold_rtf: f32,
    /// Mean of the runs after the first, or `None` when there was no second run: reporting a
    /// "warm" number derived from a single cold run would be a measurement of nothing.
    pub warm_rtf: Option<f32>,
    /// How many runs contributed to `warm_rtf`.
    pub warm_count: usize,
    /// Word-weighted WER, or `None` when the clips carried no reference transcripts.
    pub wer: Option<f32>,
    /// Total seconds of audio processed.
    pub audio_secs: f32,
    /// What the engine decided while selecting a backend -- including why it fell back, if it did.
    pub notes: Vec<String>,
}

/// Build the requested engine, measure it on real (or, failing that, synthetic) clips, and report.
///
/// Overrides are applied to a *copy* of the settings, so measuring a model the user has not
/// selected never changes what dictation uses.
fn run_benchmark_job(
    settings_path: &std::path::Path,
    model_id: Option<String>,
    backend: Option<BackendPreference>,
) -> Result<BenchReport, String> {
    let mut settings = Settings::load(settings_path).map_err(|e| e.to_string())?;
    if let Some(id) = model_id {
        settings.model_id = id;
    }
    if let Some(b) = backend {
        settings.backend = b;
    }

    let data = app_data_dir(settings_path);
    let model_dir = data.join("models").join(&settings.model_id);
    if !model_dir.exists() {
        return Err(format!(
            "model '{}' is not installed ({})",
            settings.model_id,
            model_dir.display()
        ));
    }
    let ctx = EngineInitContext {
        model_dir: model_dir.clone(),
        cache_dir: data.join("cache"),
        cpu_threads: 0,
    };

    let t0 = std::time::Instant::now();
    let mut engine = build_engine_for(&model_dir, &settings, &ctx)?;
    engine.initialize(&ctx).map_err(|e| e.to_string())?;
    let engine_load_ms = t0.elapsed().as_secs_f32() * 1000.0;

    // Three clips keeps a first NPU run tolerable; the CLI's `--quick` uses the same number.
    let (clips, source) = lw_core::bench::quick_clips(None, 3).map_err(|e| e.to_string())?;
    let clip_source = source.describe();
    let m = lw_core::bench::measure(engine.as_mut(), &clips, source, |_| {}).map_err(|e| e.to_string())?;

    let report = BenchReport {
        machine: crate::catalog::probe_capabilities().summary(),
        // Read back what ran, not what was asked for: a requested NPU run that fell back to CPU
        // must say CPU.
        backend: format!("{} on {}", engine.provider(), engine.acceleration()),
        model_id: settings.model_id.clone(),
        model_dir: model_dir.display().to_string(),
        clip_source,
        engine_load_ms,
        clips: m.results,
        cold_rtf: m.cold_rtf,
        warm_rtf: (m.warm_count > 0).then_some(m.warm_rtf),
        warm_count: m.warm_count,
        wer: m.wer,
        audio_secs: m.audio_secs,
        notes: engine.notes().to_vec(),
    };
    engine.shutdown();
    Ok(report)
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
            WorkerCmd::Describe { reply } => {
                // Deliberately does not load the engine: the honest answer before the first
                // dictation is "nothing is running yet", not a guess dressed up as a fact.
                let desc = match &loaded {
                    Some(Ok(l)) => ActiveBackend {
                        loaded: true,
                        provider: Some(l.engine.provider().to_string()),
                        acceleration: Some(l.engine.acceleration().to_string()),
                        accelerator: l.engine.accelerator().map(|a| a.id().to_string()),
                        accelerator_kind: l
                            .engine
                            .accelerator()
                            .map(|a| a.kind().label().to_ascii_lowercase()),
                        device: Some(l.engine.device().name.clone()),
                        notes: l.engine.notes().to_vec(),
                        model_id: l.settings.model_id.clone(),
                        error: None,
                    },
                    Some(Err(e)) => ActiveBackend {
                        loaded: false,
                        provider: None,
                        acceleration: None,
                        accelerator: None,
                        accelerator_kind: None,
                        device: None,
                        notes: Vec::new(),
                        model_id: String::new(),
                        error: Some(e.clone()),
                    },
                    None => ActiveBackend::default(),
                };
                let _ = reply.send(desc);
            }
            WorkerCmd::Benchmark {
                model_id,
                backend,
                reply,
            } => {
                // Drop the dictation engine first: only one engine may hold the NPU at a time.
                loaded = None;
                let _ = reply.send(run_benchmark_job(&settings_path, model_id, backend));
            }
            WorkerCmd::StartRecording { hands_free } => {
                let mut cap = Capture::new(None, 16_000 * 120);
                match cap.start() {
                    Ok(()) => {
                        if hands_free {
                            spawn_silence_watcher(app.clone(), &cap);
                        }
                        capture = Some(cap);
                    }
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

/// Hands-free mode: watch the live capture and end the utterance once the speaker stops.
///
/// Runs the model-agnostic endpoint state machine from `lw_core::vad` over the capture's ring
/// buffer. The detector is energy-based, so it needs no extra model download; because the endpoint
/// logic is behind the [`Vad`](lw_core::vad::Vad) trait, swapping in Silero later changes nothing
/// else. The watcher stops as soon as the recording leaves `Listening` (e.g. the user pressed the
/// key again first).
fn spawn_silence_watcher(app: AppHandle, capture: &Capture) {
    use lw_core::audio::TARGET_SAMPLE_RATE;
    use lw_core::vad::{EndpointConfig, EndpointDetector, EnergyVad, VAD_FRAME_SIZE, Vad};

    let ring = capture.ring();
    let native_rate = capture.native_sample_rate().max(1);
    // The ring holds native-rate samples; size the analysis hop so it is ~32 ms of real time.
    let hop = ((VAD_FRAME_SIZE as u64 * native_rate as u64) / TARGET_SAMPLE_RATE as u64).max(1) as usize;

    std::thread::Builder::new()
        .name("hands-free-vad".into())
        .spawn(move || {
            let settings = Settings::load(&app.state::<AppState>().settings_path).unwrap_or_default();
            let cfg = EndpointConfig {
                frame_ms: 1000.0 * hop as f32 / native_rate as f32,
                ..settings.vad
            };
            let mut detector = EndpointDetector::new(cfg);
            let mut vad = EnergyVad::default();
            let mut consumed = 0usize;

            loop {
                if app.state::<AppState>().recording_state() != RecordingState::Listening {
                    return; // stopped by the user or by an error
                }
                let samples = ring.snapshot();
                while consumed + hop <= samples.len() {
                    let frame = &samples[consumed..consumed + hop];
                    consumed += hop;
                    let Ok(p) = vad.process_frame(frame) else { continue };
                    for event in detector.push(p) {
                        if let lw_core::vad::EndpointEvent::SegmentComplete(_) = event {
                            let state = app.state::<AppState>();
                            if state.recording_state() == RecordingState::Listening {
                                state.transition(&app, RecordingState::Processing);
                                state.worker.send(WorkerCmd::StopRecording);
                            }
                            return;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        })
        .ok();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercise the exact path the app's Benchmark button takes, against a real installed model.
    ///
    /// Ignored by default: it needs a model on disk and takes tens of seconds (minutes on a first
    /// NPU run, which prepares and caches the HTP context). Run it explicitly with
    /// `cargo test -p localwisper --lib -- --ignored --nocapture`, optionally pointing
    /// `LW_TEST_SETTINGS` at a settings.json other than the installed app's.
    #[test]
    #[ignore = "needs an installed model; run explicitly"]
    fn benchmark_job_measures_a_real_model() {
        let settings_path = std::env::var("LW_TEST_SETTINGS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let base = std::env::var("APPDATA").expect("APPDATA");
                PathBuf::from(base)
                    .join("ai.localwisper.app")
                    .join("settings.json")
            });
        // `Settings::load` falls back to defaults when the file is absent, which is the state of
        // a fresh install - so only the model has to be there.
        let models = app_data_dir(&settings_path).join("models");
        assert!(
            models.is_dir() && models.read_dir().is_ok_and(|mut d| d.next().is_some()),
            "no model installed under {}",
            models.display()
        );

        let report = run_benchmark_job(&settings_path, None, None).expect("benchmark should run");
        println!("machine     : {}", report.machine);
        println!("backend     : {}", report.backend);
        println!("model       : {} ({})", report.model_id, report.model_dir);
        println!("clips from  : {}", report.clip_source);
        println!("engine load : {:.0} ms", report.engine_load_ms);
        for n in &report.notes {
            println!("note        : {n}");
        }
        for c in &report.clips {
            println!(
                "  {:<22} {:>7.0} ms  RTF {:.4}  WER {}",
                c.name,
                c.ms,
                c.rtf,
                c.wer.map(|w| format!("{w:.2}")).unwrap_or_else(|| "-".into())
            );
        }
        println!("cold RTF    : {:.4}", report.cold_rtf);
        println!(
            "warm RTF    : {}  over {} run(s)",
            report
                .warm_rtf
                .map(|r| format!("{r:.4}"))
                .unwrap_or_else(|| "n/a".into()),
            report.warm_count
        );
        println!(
            "WER         : {}",
            report
                .wer
                .map(|w| format!("{w:.3}"))
                .unwrap_or_else(|| "n/a".into())
        );

        assert!(
            !report.clips.is_empty(),
            "a report with no clips measures nothing"
        );
        assert!(report.cold_rtf > 0.0, "RTF must be a real timing");
        assert!(report.audio_secs > 0.0);
        // warm_rtf is Some exactly when a second run happened - the invariant the UI relies on.
        assert_eq!(report.warm_rtf.is_some(), report.warm_count > 0);
        // The backend string must describe what ran, so it can never be empty.
        assert!(!report.backend.is_empty());
    }
}
