//! The dictation worker: hotkey to captured audio to transcription to text in the focused window.
//!
//! Owns the ONNX Runtime session, the engine and the microphone capture on a thread of its own,
//! because the engine holds a `Session` that is `Send` but not `Sync` and therefore cannot live in
//! anything a UI shares. A front end holds two channel ends and nothing else.
//!
//! Lifted out of the Tauri command layer, where the same loop emitted Tauri events directly. The
//! only change is that it now reports through a channel, so a second front end can hear it. The
//! behaviour -- lazy engine load, hands-free endpointing, inject-then-clipboard delivery -- is the
//! same, and is what the Tauri build has been running.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender, unbounded};
use lw_core::audio::AudioBuffer;
use lw_core::settings::Settings;
use lw_platform::{AudioCapture, Capture};
use serde::{Deserialize, Serialize};

use crate::bench::{ActiveBackend, Loaded, build_pipeline, load_engine, meter_level};

/// Where the recording state machine is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingState {
    /// Nothing happening; overlay hidden.
    #[default]
    Idle,
    /// Hotkey held or toggled on; audio is being captured.
    Listening,
    /// Utterance ended; the engine is transcribing.
    Processing,
    /// Transcription finished and the text was delivered; transient.
    Done,
    /// Something failed; transient.
    Error,
}

impl RecordingState {
    pub fn label(self) -> &'static str {
        match self {
            RecordingState::Idle => "Idle",
            RecordingState::Listening => "Listening",
            RecordingState::Processing => "Transcribing",
            RecordingState::Done => "Done",
            RecordingState::Error => "Error",
        }
    }
}

/// What a front end asks the worker to do.
pub enum Command {
    /// Begin capturing.
    Start {
        /// End the utterance automatically once the speaker stops, rather than on a second press.
        hands_free: bool,
    },
    /// Stop capturing, transcribe, and deliver the text.
    Stop,
    /// Forget the loaded engine, so the next utterance picks up changed settings.
    ReloadSettings,
    /// Open or close a microphone stream that only feeds the level meter.
    ///
    /// The meter is otherwise alive only while dictating, which makes "is my microphone even
    /// working?" impossible to answer without dictating into something.
    MicTest(bool),
    /// Ask what the loaded engine is actually running on.
    Describe,
    Shutdown,
}

/// What the worker reports back.
#[derive(Clone, Debug)]
pub enum Event {
    State(RecordingState),
    /// Meter position in `[0, 1]`, already mapped for display.
    Level(f32),
    /// Where an utterance ended up.
    ///
    /// Four outcomes, and telling them apart is the difference between a user who knows what
    /// happened and one who pressed a key and saw nothing.
    /// Where an utterance ended up.
    ///
    /// Four outcomes, and telling them apart is the difference between a user who knows what
    /// happened and one who pressed a key and saw nothing.
    Transcript {
        text: String,
        delivery: Delivery,
        provider: String,
    },
    Error(String),
    Backend(Box<ActiveBackend>),
    /// Whether the microphone-test stream is open, or why it could not be opened.
    MicTest(Result<bool, String>),
}

/// Where a transcript went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Typed into another application's focused control.
    Injected,
    /// This application's own window had focus, so the text is handed to the front end to put
    /// where the user was typing.
    ///
    /// Pasting into our own window means synthesizing Ctrl+V for a thread in this very process and
    /// then restoring the clipboard 300 ms later -- while that thread is the busiest one here,
    /// having just finished a transcription and a redraw. Losing that race pastes whatever was on
    /// the clipboard before. The front end owns the text box; it can simply put the text in it.
    OwnWindow,
    /// Nowhere it could be typed, so it is on the clipboard and the user can paste it.
    Clipboard,
    /// The recording produced no words. Not a failure, and not something to show as an empty
    /// transcript either.
    Nothing,
}

/// The front end's handle: send commands, receive events.
pub struct Handle {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
    state: Arc<std::sync::Mutex<RecordingState>>,
}

impl Handle {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    /// The current state, for a front end that draws rather than reacts.
    pub fn state(&self) -> RecordingState {
        self.state.lock().map(|s| *s).unwrap_or_default()
    }

    /// A non-owning way to drive the worker from somewhere else.
    ///
    /// `Handle` shuts the worker down when it is dropped, which is right for the one owner and
    /// wrong for everybody else -- the hotkey pump holding a clone would kill the worker the
    /// moment the pump was rebound. A `Remote` can send and can read the state, and dropping it
    /// does nothing.
    pub fn remote(&self) -> Remote {
        Remote {
            tx: self.tx.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

/// A borrowed view of a running worker: send commands, read the state, no ownership.
#[derive(Clone)]
pub struct Remote {
    tx: Sender<Command>,
    state: Arc<std::sync::Mutex<RecordingState>>,
}

impl Remote {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn state(&self) -> RecordingState {
        self.state.lock().map(|s| *s).unwrap_or_default()
    }

    /// A `Remote` attached to a plain channel instead of a worker, for tests.
    ///
    /// The hotkey pump's whole job is to put the right `Command` on the worker's channel, and the
    /// only way to watch it do that on a real worker would be to let it open the microphone. The
    /// second return is the channel the pump's commands arrive on, and the handle to set the state
    /// the pump will read back.
    #[cfg(test)]
    pub(crate) fn detached() -> (
        Remote,
        crossbeam_channel::Receiver<Command>,
        Arc<std::sync::Mutex<RecordingState>>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let state = Arc::new(std::sync::Mutex::new(RecordingState::Idle));
        (
            Remote {
                tx,
                state: Arc::clone(&state),
            },
            rx,
            state,
        )
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

/// Whether a microphone-test stream is currently open, so its pump knows when to stop.
static MIC_TEST_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Start the worker thread.
pub fn spawn(settings_path: PathBuf) -> Handle {
    let (tx, rx) = unbounded::<Command>();
    let (etx, erx) = unbounded::<Event>();
    let state = Arc::new(std::sync::Mutex::new(RecordingState::Idle));
    let worker_state = Arc::clone(&state);
    let self_tx = tx.clone();

    std::thread::Builder::new()
        .name("lw-dictation".into())
        .spawn(move || worker_loop(settings_path, rx, etx, worker_state, self_tx))
        .expect("spawn dictation worker");

    Handle {
        tx,
        events: erx,
        state,
    }
}

struct Ctx {
    events: Sender<Event>,
    state: Arc<std::sync::Mutex<RecordingState>>,
}

impl Ctx {
    fn set(&self, next: RecordingState) {
        if let Ok(mut s) = self.state.lock() {
            *s = next;
        }
        let _ = self.events.send(Event::State(next));
    }

    /// Report a failure and return to rest, so a front end never has to guess whether the worker
    /// is still busy after an error.
    fn fail(&self, msg: &str) {
        tracing::error!("dictation worker: {msg}");
        self.set(RecordingState::Error);
        let _ = self.events.send(Event::Error(msg.to_string()));
        self.set(RecordingState::Idle);
    }
}

fn worker_loop(
    settings_path: PathBuf,
    rx: Receiver<Command>,
    events: Sender<Event>,
    state: Arc<std::sync::Mutex<RecordingState>>,
    self_tx: Sender<Command>,
) {
    let ctx = Ctx { events, state };
    // Lazy-load the engine on first use so startup is not blocked by a 650 MB model.
    let mut loaded: Option<Result<Loaded, String>> = None;
    let mut capture: Option<Capture> = None;
    // A second capture used only to drive the level meter while the user checks their microphone.
    let mut mic_test: Option<Capture> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Shutdown => break,
            Command::ReloadSettings => loaded = None,
            Command::Describe => {
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
                        error: Some(e.clone()),
                        ..ActiveBackend::default()
                    },
                    None => ActiveBackend::default(),
                };
                let _ = ctx.events.send(Event::Backend(Box::new(desc)));
            }
            Command::Start { hands_free } => {
                let device = capture_device(&settings_path);
                let mut cap = Capture::new(device, 16_000 * 120);
                match cap.start() {
                    Ok(()) => {
                        ctx.set(RecordingState::Listening);
                        if hands_free {
                            spawn_silence_watcher(
                                &cap,
                                &settings_path,
                                Arc::clone(&ctx.state),
                                self_tx.clone(),
                            );
                        }
                        spawn_level_pump(
                            &cap,
                            LevelSource::Recording,
                            Arc::clone(&ctx.state),
                            ctx.events.clone(),
                        );
                        capture = Some(cap);
                    }
                    Err(e) => ctx.fail(&format!("microphone: {e}")),
                }
            }
            Command::MicTest(enabled) => {
                if enabled {
                    if mic_test.is_none() {
                        // A separate capture from dictation's: this one exists only to drive the
                        // meter, and whatever it hears is dropped rather than transcribed.
                        let device = capture_device(&settings_path);
                        let mut cap = Capture::new(device, 16_000 * 4);
                        match cap.start() {
                            Ok(()) => {
                                spawn_level_pump(
                                    &cap,
                                    LevelSource::MicTest,
                                    Arc::clone(&ctx.state),
                                    ctx.events.clone(),
                                );
                                mic_test = Some(cap);
                                let _ = ctx.events.send(Event::MicTest(Ok(true)));
                            }
                            Err(e) => {
                                let _ = ctx
                                    .events
                                    .send(Event::MicTest(Err(format!("microphone: {e}"))));
                            }
                        }
                    } else {
                        let _ = ctx.events.send(Event::MicTest(Ok(true)));
                    }
                } else {
                    if let Some(mut cap) = mic_test.take() {
                        let _ = cap.stop();
                    }
                    MIC_TEST_ACTIVE.store(false, Ordering::Relaxed);
                    // Park the meter at zero so it does not freeze at the last value it saw.
                    let _ = ctx.events.send(Event::Level(0.0));
                    let _ = ctx.events.send(Event::MicTest(Ok(false)));
                }
            }
            Command::Stop => {
                let Some(mut cap) = capture.take() else {
                    continue;
                };
                ctx.set(RecordingState::Processing);
                let audio: AudioBuffer = match cap.stop() {
                    Ok(a) => a,
                    Err(e) => {
                        ctx.fail(&format!("capture stop: {e}"));
                        continue;
                    }
                };
                if loaded.is_none() {
                    loaded = Some(load_engine(&settings_path));
                }
                let Some(Ok(engine_state)) = loaded.as_mut() else {
                    let msg = loaded
                        .as_ref()
                        .and_then(|r| r.as_ref().err())
                        .cloned()
                        .unwrap_or_else(|| "engine unavailable".into());
                    ctx.fail(&msg);
                    continue;
                };
                let provider = engine_state.engine.provider().to_string();
                let transcript = match engine_state.engine.transcribe(&audio) {
                    Ok(t) => t,
                    Err(e) => {
                        ctx.fail(&format!("transcribe: {e}"));
                        continue;
                    }
                };
                let pipeline = build_pipeline(&engine_state.settings);
                let text = pipeline.run(&transcript.text);
                let delivery = deliver(&text);
                let _ = ctx.events.send(Event::Transcript {
                    text,
                    delivery,
                    provider,
                });
                ctx.set(RecordingState::Done);
                ctx.set(RecordingState::Idle);
            }
        }
    }
}

/// The configured input device, or `None` for the system default.
fn capture_device(settings_path: &std::path::Path) -> Option<String> {
    Settings::load(settings_path)
        .ok()
        .map(|s| s.audio.input_device)
        .filter(|d| !d.trim().is_empty())
}

/// Put the text where the user was typing.
///
/// Three destinations in order of preference: our own window, somebody else's window, the
/// clipboard. Falling back to the clipboard rather than dropping the text is the point -- a
/// transcription that cannot be delivered is still the user's words.
fn deliver(text: &str) -> Delivery {
    if text.trim().is_empty() {
        return Delivery::Nothing;
    }
    // Our own window needs no keystrokes and no clipboard: the front end can put the text in the
    // control the user is typing in. Doing it the other way round is a race this process holds
    // both ends of, and it loses it often enough to look like the hotkey not working.
    if lw_platform::platform().foreground_is_own_process() {
        return Delivery::OwnWindow;
    }
    let platform = lw_platform::platform();
    if let Ok(mut injector) = platform.injector()
        && injector.inject(text).is_ok()
    {
        return Delivery::Injected;
    }
    if let Ok(mut clip) = platform.clipboard() {
        let _ = clip.set_text(text);
    }
    Delivery::Clipboard
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LevelSource {
    Recording,
    MicTest,
}

/// Push the capture's real RMS to the front end until its stream ends.
///
/// This replaced a stub that generated a sine wave: the meter moved convincingly while showing
/// nothing about the microphone, which is precisely the kind of thing this project must not do.
fn spawn_level_pump(
    capture: &Capture,
    source: LevelSource,
    state: Arc<std::sync::Mutex<RecordingState>>,
    events: Sender<Event>,
) {
    let level = capture.level_handle();
    if source == LevelSource::MicTest {
        MIC_TEST_ACTIVE.store(true, Ordering::Relaxed);
    }
    std::thread::Builder::new()
        .name("lw-level".into())
        .spawn(move || {
            loop {
                let keep_going = match source {
                    LevelSource::Recording => {
                        state.lock().map(|s| *s).unwrap_or_default() == RecordingState::Listening
                    }
                    LevelSource::MicTest => MIC_TEST_ACTIVE.load(Ordering::Relaxed),
                };
                if !keep_going {
                    break;
                }
                if events.send(Event::Level(meter_level(level.get()))).is_err() {
                    return; // the front end is gone
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // Leave the meter at rest rather than frozen at the last sample.
            let _ = events.send(Event::Level(0.0));
        })
        .ok();
}

/// Hands-free mode: watch the live capture and end the utterance once the speaker stops.
///
/// Runs the model-agnostic endpoint state machine from `lw_core::vad` over the capture's ring
/// buffer. The detector is energy-based, so it needs no extra model download; because the endpoint
/// logic is behind the `Vad` trait, swapping in Silero later changes nothing else. The watcher
/// stops as soon as the recording leaves `Listening` -- for instance because the user pressed the
/// key again first.
fn spawn_silence_watcher(
    capture: &Capture,
    settings_path: &std::path::Path,
    state: Arc<std::sync::Mutex<RecordingState>>,
    commands: Sender<Command>,
) {
    use lw_core::audio::TARGET_SAMPLE_RATE;
    use lw_core::vad::{EndpointConfig, EndpointDetector, EnergyVad, VAD_FRAME_SIZE, Vad};

    let ring = capture.ring();
    let native_rate = capture.native_sample_rate().max(1);
    // The ring holds native-rate samples; size the analysis hop so it is ~32 ms of real time.
    let hop =
        ((VAD_FRAME_SIZE as u64 * native_rate as u64) / TARGET_SAMPLE_RATE as u64).max(1) as usize;
    let settings_path = settings_path.to_path_buf();

    std::thread::Builder::new()
        .name("hands-free-vad".into())
        .spawn(move || {
            let settings = Settings::load(&settings_path).unwrap_or_default();
            let cfg = EndpointConfig {
                frame_ms: 1000.0 * hop as f32 / native_rate as f32,
                ..settings.vad
            };
            let mut detector = EndpointDetector::new(cfg);
            let mut vad = EnergyVad::default();
            let mut consumed = 0usize;

            loop {
                if state.lock().map(|s| *s).unwrap_or_default() != RecordingState::Listening {
                    return; // stopped by the user or by an error
                }
                let samples = ring.snapshot();
                while consumed + hop <= samples.len() {
                    let frame = &samples[consumed..consumed + hop];
                    consumed += hop;
                    let Ok(p) = vad.process_frame(frame) else {
                        continue;
                    };
                    for event in detector.push(p) {
                        if let lw_core::vad::EndpointEvent::SegmentComplete(_) = event {
                            let _ = commands.send(Command::Stop);
                            return;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        })
        .ok();
}
