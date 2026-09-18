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
    /// Open or close a microphone stream that feeds the level meter and transcribes nothing.
    ///
    /// The meter is otherwise only alive while dictating, which makes "is my microphone even
    /// working?" impossible to answer without dictating into something.
    MicTest {
        /// True to open the stream, false to close it.
        enabled: bool,
        /// Whether the stream is open afterwards, or why it could not be opened.
        reply: Sender<Result<bool, String>>,
    },
    /// Report what the loaded engine is actually running on, without loading one.
    Describe {
        /// Where to send the description.
        reply: Sender<ActiveBackend>,
    },
    /// Measure a model on **every usable accelerator** and reply with the comparison.
    ///
    /// The point is to answer "which should I use here" in one action rather than making the
    /// user run, record and compare each backend by hand.
    BenchmarkAll {
        /// Model to measure; `None` means whatever Settings currently selects.
        model_id: Option<String>,
        /// Where to send the suite.
        reply: Sender<Result<BenchSuite, String>>,
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

    /// A handle attached to nothing, for tests that need an [`AppState`] but no worker thread.
    /// Commands sent to it are dropped, which is what `send` already does once a worker stops.
    #[cfg(test)]
    pub fn detached() -> Self {
        let (tx, _rx) = crossbeam_channel::unbounded();
        Self { tx }
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

/// How many fixture clips a benchmark uses.
///
/// The whole committed set is fifteen (three each in en/ru/es/uk/zh). Taking all of them costs a few
/// seconds per backend -- engine load dominates a run, not transcription -- and in exchange the
/// WER covers four languages rather than only English. `measure` scores only the languages the
/// model claims, so an English-only model is still judged on English alone.
const BENCH_CLIPS: usize = 15;

/// A benchmark report: every number in it was measured on this machine by this run.
#[derive(Clone, Debug, serde::Serialize)]
pub struct BenchReport {
    /// One-line description of the machine.
    pub machine: String,
    /// The backend that **actually** executed, read back from the engine rather than requested.
    pub backend: String,
    /// That backend's stable accelerator id, for comparing runs without matching display prose.
    pub accelerator: Option<String>,
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
    /// Token-weighted error rate, or `None` when the clips carried no reference transcripts **or**
    /// they spanned both words and characters -- see `mixed_units`.
    pub wer: Option<f32>,
    /// What `wer` is measured in: `"word"` or `"character"`, when there is a figure.
    pub unit: Option<&'static str>,
    /// True when the scored clips mixed word-scored and character-scored languages, so no single
    /// figure exists. Chinese is not measured in words and Russian is not measured in characters,
    /// and averaging the two would produce a number with no unit.
    pub mixed_units: bool,
    /// Total seconds of audio processed.
    pub audio_secs: f32,
    /// What the engine decided while selecting a backend -- including why it fell back, if it did.
    pub notes: Vec<String>,
}

/// One accelerator that was offered but not measured, and why.
#[derive(Clone, Debug, serde::Serialize)]
pub struct BenchSkipped {
    /// Stable accelerator id.
    pub accelerator: String,
    /// Human label.
    pub label: String,
    /// Why it was skipped or how it failed.
    pub reason: String,
}

/// Every accelerator measured on the same clips, so the numbers can honestly be compared.
#[derive(Clone, Debug, serde::Serialize)]
pub struct BenchSuite {
    /// One-line description of the machine.
    pub machine: String,
    /// The model every run used.
    pub model_id: String,
    /// Where the audio came from. One clip set for all runs -- that is what makes this a comparison.
    pub clip_source: String,
    /// One report per accelerator that ran, in the order they were measured.
    pub runs: Vec<BenchReport>,
    /// Accelerators that could not be measured, with the reason.
    pub skipped: Vec<BenchSkipped>,
    /// The id of the fastest run by warm RTF, or `None` if nothing ran.
    pub fastest: Option<String>,
    /// The id of the most accurate run by word-weighted WER, when WER was computable.
    pub most_accurate: Option<String>,
}

/// Measure every usable accelerator on one clip set and return the comparison.
///
/// Emits `benchmark_progress` as each accelerator starts and finishes, because a full sweep takes
/// minutes -- a first NPU run alone can spend several preparing its context binary.
fn run_benchmark_suite(
    settings_path: &std::path::Path,
    model_id: Option<String>,
    mut on_progress: impl FnMut(serde_json::Value),
) -> Result<BenchSuite, String> {
    let settings = Settings::load(settings_path).map_err(|e| e.to_string())?;
    let model = model_id.unwrap_or(settings.model_id);
    let caps = crate::catalog::probe_capabilities();

    let runtime_dir = runtime_dir().ok_or_else(|| "ONNX Runtime not found".to_string())?;
    let runtime = lw_ort::OrtRuntime::init(&runtime_dir).map_err(|e| e.to_string())?;
    let usable = runtime.usable_accelerators();
    if usable.is_empty() {
        return Err("no usable accelerator on this machine".into());
    }

    // Load once. Every backend must see identical audio or the comparison means nothing.
    let clips = lw_core::bench::quick_clips(None, BENCH_CLIPS).map_err(|e| e.to_string())?;
    let clip_source = clips.1.describe();
    let total = usable.len();

    // What the catalog says about this model, so an accelerator it cannot possibly use is skipped
    // rather than attempted. Without the entry we try everything, which is the old behaviour and
    // the right fallback: guessing "unsupported" from a missing catalog row would hide a model
    // that works.
    let entry = lw_core::model::Catalog::builtin()
        .ok()
        .and_then(|c| c.get(&model).cloned());

    let mut runs = Vec::new();
    let mut skipped = Vec::new();
    for (i, accel) in usable.iter().enumerate() {
        if let Some(reason) = entry.as_ref().and_then(|e| e.unsupported_on(*accel)) {
            on_progress(serde_json::json!({
                "index": i, "total": total,
                "accelerator": accel.id(), "label": accel.label(),
                "stage": "skipped", "reason": reason.clone(),
            }));
            skipped.push(BenchSkipped {
                accelerator: accel.id().to_string(),
                label: accel.label().to_string(),
                reason,
            });
            continue;
        }
        on_progress(serde_json::json!({
            "index": i, "total": total,
            "accelerator": accel.id(), "label": accel.label(),
            "stage": "running",
        }));
        let pref = lw_core::engine::BackendPreference::for_accelerator(*accel);
        match run_benchmark_job(settings_path, Some(model.clone()), Some(pref), Some(&clips)) {
            Ok(report) => {
                on_progress(serde_json::json!({
                    "index": i, "total": total,
                    "accelerator": accel.id(), "label": accel.label(),
                    "stage": "done",
                    "warm_rtf": report.warm_rtf, "wer": report.wer,
                }));
                runs.push(report);
            }
            Err(reason) => {
                on_progress(serde_json::json!({
                    "index": i, "total": total,
                    "accelerator": accel.id(), "label": accel.label(),
                    "stage": "failed", "reason": reason.clone(),
                }));
                skipped.push(BenchSkipped {
                    accelerator: accel.id().to_string(),
                    label: accel.label().to_string(),
                    reason,
                });
            }
        }
    }

    // "Fastest" uses the warm figure: the cold run carries one-time setup that says nothing about
    // steady-state dictation. A run with no warm figure is not eligible rather than being ranked
    // on a number that means something else.
    let fastest = runs
        .iter()
        .filter_map(|r| r.warm_rtf.map(|w| (r, w)))
        .filter(|(_, w)| w.is_finite() && *w > 0.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .and_then(|(r, _)| r.accelerator.clone());
    let most_accurate = runs
        .iter()
        .filter_map(|r| r.wer.map(|w| (r, w)))
        .filter(|(_, w)| w.is_finite())
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .and_then(|(r, _)| r.accelerator.clone());

    Ok(BenchSuite {
        machine: caps.summary(),
        model_id: model,
        clip_source,
        runs,
        skipped,
        fastest,
        most_accurate,
    })
}

/// Build the requested engine, measure it on real (or, failing that, synthetic) clips, and report.
///
/// Overrides are applied to a *copy* of the settings, so measuring a model the user has not
/// selected never changes what dictation uses.
fn run_benchmark_job(
    settings_path: &std::path::Path,
    model_id: Option<String>,
    backend: Option<BackendPreference>,
    clips: Option<&(Vec<lw_core::bench::Clip>, lw_core::bench::ClipSource)>,
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

    // The whole fixture set. Transcription is not what makes a sweep slow -- loading an engine
    // is, and a first NPU run additionally prepares a context binary. At the measured rates,
    // twelve clips instead of three costs a few seconds per backend and buys a WER over four
    // languages instead of three English clips.
    // Comparing backends is only meaningful on identical audio, so a suite loads the clips once
    // and hands the same set to every run.
    let owned;
    let (clips, source) = match clips {
        Some(c) => (&c.0, c.1.clone()),
        None => {
            owned = lw_core::bench::quick_clips(None, BENCH_CLIPS).map_err(|e| e.to_string())?;
            (&owned.0, owned.1.clone())
        }
    };
    let clip_source = source.describe();
    let m = lw_core::bench::measure(engine.as_mut(), clips, source, |_| {}).map_err(|e| e.to_string())?;

    let report = BenchReport {
        machine: crate::catalog::probe_capabilities().summary(),
        // Read back what ran, not what was asked for: a requested NPU run that fell back to CPU
        // must say CPU.
        backend: format!("{} on {}", engine.provider(), engine.acceleration()),
        accelerator: engine.accelerator().map(|a| a.id().to_string()),
        model_id: settings.model_id.clone(),
        model_dir: model_dir.display().to_string(),
        clip_source,
        engine_load_ms,
        clips: m.results,
        cold_rtf: m.cold_rtf,
        warm_rtf: (m.warm_count > 0).then_some(m.warm_rtf),
        warm_count: m.warm_count,
        wer: m.wer,
        unit: m.unit.map(lw_core::bench::ErrorUnit::label),
        mixed_units: m.mixed_units,
        audio_secs: m.audio_secs,
        notes: engine.notes().to_vec(),
    };
    engine.shutdown();
    Ok(report)
}

/// Whether a microphone-test stream is currently open, so its pump knows when to stop.
static MIC_TEST_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Which stream a level pump is reading, and therefore what makes it stop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LevelSource {
    /// Dictation: runs while the state machine says we are listening.
    Recording,
    /// The user checking their microphone: runs until they turn it off.
    MicTest,
}

/// Map an RMS amplitude to a meter position in `[0, 1]`.
///
/// A linear RMS bar is useless: ordinary speech sits around 0.02-0.2, so it would barely leave
/// the left edge while clipping would look like a third of the scale. Hearing is closer to
/// logarithmic, so this uses the usual voice-meter range, -60 dBFS at the left to 0 dBFS at the
/// right. It is a display mapping of a real measurement, not a substitute for one.
fn meter_level(rms: f32) -> f32 {
    const FLOOR_DB: f32 = -60.0;
    if !rms.is_finite() || rms <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * rms.clamp(1e-6, 1.0).log10();
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

/// Push the capture's real RMS to the UI's level channel until its stream ends.
///
/// This replaced a stub that generated a sine wave: the meter moved convincingly while showing
/// nothing about the microphone, which is precisely the kind of thing this project must not do.
fn spawn_level_pump(app: AppHandle, capture: &Capture, source: LevelSource) {
    let level = capture.level_handle();
    if source == LevelSource::MicTest {
        MIC_TEST_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    std::thread::Builder::new()
        .name("lw-level".into())
        .spawn(move || {
            loop {
                let keep_going = match source {
                    LevelSource::Recording => {
                        app.state::<AppState>().recording_state() == RecordingState::Listening
                    }
                    LevelSource::MicTest => MIC_TEST_ACTIVE.load(std::sync::atomic::Ordering::Relaxed),
                };
                if !keep_going {
                    break;
                }
                let display = meter_level(level.get());
                if let Some(channel) = app.state::<AppState>().mic_level.lock().as_ref() {
                    let _ = channel.send(display);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // Leave the meter at rest rather than frozen at the last sample.
            if let Some(channel) = app.state::<AppState>().mic_level.lock().as_ref() {
                let _ = channel.send(0.0);
            }
        })
        .ok();
}

fn worker_loop(app: AppHandle, settings_path: PathBuf, rx: Receiver<WorkerCmd>) {
    // Lazy-load the engine on first use so app startup is not blocked by the 650 MB model.
    let mut loaded: Option<Result<Loaded, String>> = None;
    let mut capture: Option<Capture> = None;
    // A second capture used only to drive the level meter while the user checks their mic.
    let mut mic_test: Option<Capture> = None;

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
            WorkerCmd::BenchmarkAll { model_id, reply } => {
                loaded = None;
                let app2 = app.clone();
                let _ = reply.send(run_benchmark_suite(&settings_path, model_id, move |p| {
                    let _ = app2.emit("benchmark_progress", p);
                }));
            }
            WorkerCmd::Benchmark {
                model_id,
                backend,
                reply,
            } => {
                // Drop the dictation engine first: only one engine may hold the NPU at a time.
                loaded = None;
                let _ = reply.send(run_benchmark_job(&settings_path, model_id, backend, None));
            }
            WorkerCmd::StartRecording { hands_free } => {
                let device = app.state::<AppState>().capture_device();
                let mut cap = Capture::new(device, 16_000 * 120);
                match cap.start() {
                    Ok(()) => {
                        if hands_free {
                            spawn_silence_watcher(app.clone(), &cap);
                        }
                        spawn_level_pump(app.clone(), &cap, LevelSource::Recording);
                        capture = Some(cap);
                    }
                    Err(e) => emit_error(&app, &format!("microphone: {e}")),
                }
            }
            WorkerCmd::MicTest { enabled, reply } => {
                if enabled {
                    // A separate capture from dictation's: this one exists only to drive the
                    // meter, and whatever it hears is dropped rather than transcribed.
                    if mic_test.is_none() {
                        let device = app.state::<AppState>().capture_device();
                        let mut cap = Capture::new(device, 16_000 * 4);
                        match cap.start() {
                            Ok(()) => {
                                spawn_level_pump(app.clone(), &cap, LevelSource::MicTest);
                                mic_test = Some(cap);
                                let _ = reply.send(Ok(true));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(format!("microphone: {e}")));
                            }
                        }
                    } else {
                        let _ = reply.send(Ok(true));
                    }
                } else {
                    if let Some(mut cap) = mic_test.take() {
                        let _ = cap.stop();
                    }
                    MIC_TEST_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
                    // Park the meter at zero so it does not freeze at the last value it saw.
                    if let Some(channel) = app.state::<AppState>().mic_level.lock().as_ref() {
                        let _ = channel.send(0.0);
                    }
                    let _ = reply.send(Ok(false));
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

    /// Run the full comparison the Benchmark tab's "compare all" offers, on real hardware.
    ///
    /// Ignored by default: it needs an installed model and measures every usable accelerator. Run
    /// with `cargo test -p localwisper --lib -- --ignored --nocapture`.
    ///
    /// Note: the test binary exits with STATUS_STACK_BUFFER_OVERRUN *after* reporting success,
    /// because the WebGPU provider crashes on library detach and `cargo test` owns `main`, so it
    /// cannot use `lw_ort::exit_without_teardown` the way the CLI and the app do. The test result
    /// above that line is the real one.
    #[test]
    #[ignore = "needs an installed model; run explicitly"]
    fn benchmark_suite_compares_every_usable_backend() {
        let settings_path = std::env::var("LW_TEST_SETTINGS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let base = std::env::var("APPDATA").expect("APPDATA");
                PathBuf::from(base)
                    .join("ai.localwisper.app")
                    .join("settings.json")
            });
        let models = app_data_dir(&settings_path).join("models");
        assert!(
            models.is_dir() && models.read_dir().is_ok_and(|mut d| d.next().is_some()),
            "no model installed under {}",
            models.display()
        );

        let mut progress = Vec::new();
        let suite = run_benchmark_suite(&settings_path, None, |p| {
            println!("  progress: {p}");
            progress.push(p);
        })
        .expect("the suite should run");

        println!("machine : {}", suite.machine);
        println!("model   : {}", suite.model_id);
        println!("clips   : {}", suite.clip_source);
        println!();
        println!(
            "{:<28} {:>10} {:>10} {:>8}",
            "BACKEND", "COLD RTF", "WARM RTF", "WER"
        );
        for r in &suite.runs {
            println!(
                "{:<28} {:>10.4} {:>10} {:>8}",
                r.backend,
                r.cold_rtf,
                r.warm_rtf
                    .map(|w| format!("{w:.4}"))
                    .unwrap_or_else(|| "n/a".into()),
                r.wer.map(|w| format!("{w:.3}")).unwrap_or_else(|| "n/a".into()),
            );
        }
        for s in &suite.skipped {
            println!("{:<28} skipped: {}", s.label, s.reason);
        }
        println!();
        println!("fastest       : {:?}", suite.fastest);
        println!("most accurate : {:?}", suite.most_accurate);

        assert!(!suite.runs.is_empty(), "nothing was measured");
        // Every run must have used the same audio, or the comparison is meaningless.
        let clip_names: Vec<Vec<&str>> = suite
            .runs
            .iter()
            .map(|r| r.clips.iter().map(|c| c.name.as_str()).collect())
            .collect();
        assert!(
            clip_names.windows(2).all(|w| w[0] == w[1]),
            "backends were measured on different clips: {clip_names:?}"
        );
        // Progress must cover every accelerator that was offered.
        assert_eq!(progress.len(), (suite.runs.len() + suite.skipped.len()) * 2);
        // The winner must be one of the runs, not invented.
        if let Some(f) = &suite.fastest {
            assert!(suite.runs.iter().any(|r| r.accelerator.as_ref() == Some(f)));
        }
    }

    #[test]
    fn the_meter_maps_silence_to_zero_and_full_scale_to_one() {
        assert_eq!(meter_level(0.0), 0.0);
        assert_eq!(meter_level(-1.0), 0.0, "a nonsensical level must not wrap around");
        assert_eq!(meter_level(f32::NAN), 0.0);
        assert!((meter_level(1.0) - 1.0).abs() < 1e-6);
        assert!(meter_level(2.0) <= 1.0, "must clamp rather than overflow the bar");
    }

    #[test]
    fn the_meter_puts_ordinary_speech_in_the_usable_middle() {
        // The reason this mapping exists: on a linear scale, speech at RMS 0.02-0.2 would sit in
        // the leftmost fifth of the bar and look like nothing was happening.
        let quiet = meter_level(0.02);
        let normal = meter_level(0.08);
        let loud = meter_level(0.3);
        assert!(quiet > 0.2, "quiet speech should still be visible, got {quiet}");
        assert!(normal > quiet && loud > normal, "must stay monotonic");
        assert!(loud < 1.0, "loud speech must leave headroom before the top");
    }

    #[test]
    fn the_meter_is_monotonic() {
        let mut previous = -1.0;
        for step in 0..=100 {
            let level = meter_level(step as f32 / 100.0);
            assert!(level >= previous, "went backwards at {step}");
            previous = level;
        }
    }

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

        let report = run_benchmark_job(&settings_path, None, None, None).expect("benchmark should run");
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
