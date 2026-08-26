//! Shared app state: the recording-state machine surfaced to the UI via the
//! `state_changed` event, plus the mic-level IPC channel and settings location.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};

/// The dictation state machine as shown in the UI.
///
/// Serialized lowercase so the `state_changed` payload is
/// `{"state": "idle" | "listening" | "processing" | "done" | "error"}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingState {
    /// Nothing happening; overlay hidden.
    #[default]
    Idle,
    /// Hotkey held / toggled on; audio would be captured here.
    Listening,
    /// Utterance ended; the engine would be transcribing.
    Processing,
    /// Transcription finished (text injected); transient.
    Done,
    /// Something failed; transient.
    Error,
}

/// Payload of the `state_changed` event.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct StatePayload {
    /// The new state.
    pub state: RecordingState,
}

/// Global state managed by Tauri.
pub struct AppState {
    /// Where `lw_core::settings::Settings` persists (app_data_dir/settings.json).
    pub settings_path: PathBuf,
    /// Whether the overlay window should be shown while not idle (mirrors settings.overlay_enabled).
    pub overlay_enabled: AtomicBool,
    /// Mic-level stream registered by the frontend via `subscribe_mic_level`.
    pub mic_level: Mutex<Option<Channel<f32>>>,
    /// Handle to the dictation worker (audio capture + engine on a background thread).
    pub worker: crate::worker::WorkerHandle,
    recording: Mutex<RecordingState>,
    /// Bumped on every transition; lets delayed transitions detect staleness.
    generation: AtomicU64,
}

impl AppState {
    /// Create the state with the resolved settings path and the dictation worker handle.
    pub fn new(settings_path: PathBuf, overlay_enabled: bool, worker: crate::worker::WorkerHandle) -> Self {
        Self {
            settings_path,
            overlay_enabled: AtomicBool::new(overlay_enabled),
            mic_level: Mutex::new(None),
            worker,
            recording: Mutex::new(RecordingState::Idle),
            generation: AtomicU64::new(0),
        }
    }

    /// Current recording state.
    pub fn recording_state(&self) -> RecordingState {
        *self.recording.lock()
    }

    /// Current transition generation.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Transition unconditionally: set state, bump the generation, emit `state_changed`
    /// to every window, and sync overlay visibility. Returns the new generation.
    pub fn transition(&self, app: &AppHandle, next: RecordingState) -> u64 {
        *self.recording.lock() = next;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = app.emit("state_changed", StatePayload { state: next });
        self.sync_overlay(app, next);
        generation
    }

    /// Transition only if `expected_generation` is still current (i.e. no newer transition
    /// happened in between). Used by delayed transitions so they never clobber a fresh press.
    pub fn transition_if_current(
        &self,
        app: &AppHandle,
        expected_generation: u64,
        next: RecordingState,
    ) -> Option<u64> {
        let mut recording = self.recording.lock();
        if self.generation.load(Ordering::SeqCst) != expected_generation {
            return None;
        }
        *recording = next;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        drop(recording);
        let _ = app.emit("state_changed", StatePayload { state: next });
        self.sync_overlay(app, next);
        Some(generation)
    }

    /// Show the overlay while not idle (if enabled), hide it when idle.
    fn sync_overlay(&self, app: &AppHandle, state: RecordingState) {
        let Some(overlay) = app.get_webview_window("overlay") else {
            return;
        };
        let show = self.overlay_enabled.load(Ordering::Relaxed) && !matches!(state, RecordingState::Idle);
        if show {
            let _ = overlay.show();
        } else {
            let _ = overlay.hide();
        }
    }
}
