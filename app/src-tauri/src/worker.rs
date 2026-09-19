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
use lw_core::engine::BackendPreference;
use lw_core::settings::Settings;
use lw_platform::{AudioCapture, Capture};
use tauri::{AppHandle, Emitter, Manager};

// The engine, benchmark and metering core now lives in `lw-app` so a second front end can call
// it. Re-exported here because the Tauri command layer's callers still refer to these by the path
// they have always had, and because a `pub use` is a smaller change than touching every site.
pub use lw_app::bench::{
    ActiveBackend, BENCH_CLIPS, BenchReport, BenchSkipped, BenchSuite, Loaded, app_data_dir,
    build_pipeline, load_engine, meter_level, run_benchmark_job, run_benchmark_suite,
};

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
