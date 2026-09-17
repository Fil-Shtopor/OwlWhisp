//! Shared app state: the recording-state machine surfaced to the UI via the
//! `state_changed` event, plus the mic-level IPC channel and settings location.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use lw_core::settings::{HotkeyMode, Settings};
use lw_core::sound::{Cue, SoundTheme};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::Shortcut;

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
    /// Cancellation tokens for model downloads in flight, keyed by catalog id.
    pub installs: Mutex<std::collections::HashMap<String, lw_core::model::CancellationToken>>,
    /// Cue settings, mirrored from `settings.json` so a state transition never touches the disk.
    cue: Mutex<CueSettings>,
    /// The configured input device, mirrored for the same reason: opening the microphone is on
    /// the path between pressing the hotkey and hearing yourself, so it must not read the disk.
    /// `None` means "the system default".
    capture_device: Mutex<Option<String>>,
    recording: Mutex<RecordingState>,
    /// Bumped on every transition; lets delayed transitions detect staleness.
    generation: AtomicU64,
    /// The dictation shortcut currently registered with the OS, so it can be unregistered on change
    /// and so the handler ignores events from any other binding.
    shortcut: Mutex<Option<Shortcut>>,
    /// Whether [`Self::shortcut`] is actually held by the OS. It is stored separately because the
    /// requested binding is recorded even when registration fails -- the handler still has to
    /// recognise it -- and the UI must not be told a dead binding is live.
    shortcut_registered: AtomicBool,
    /// Hold-to-talk vs tap-to-toggle vs hands-free, mirroring `Settings.hotkey.mode`.
    hotkey_mode: Mutex<HotkeyMode>,
}

/// The parts of `Settings` the audible cues need, kept in memory.
#[derive(Clone, Copy, Debug)]
pub struct CueSettings {
    /// Whether cues play at all.
    pub enabled: bool,
    /// Which sound.
    pub theme: SoundTheme,
    /// Volume in `[0, 1]`.
    pub volume: f32,
}

impl Default for CueSettings {
    fn default() -> Self {
        let s = Settings::default();
        Self {
            enabled: s.sounds_enabled,
            theme: s.sound_theme,
            volume: s.sound_volume,
        }
    }
}

impl From<&Settings> for CueSettings {
    fn from(s: &Settings) -> Self {
        Self {
            enabled: s.sounds_enabled,
            theme: s.sound_theme,
            volume: s.sound_volume,
        }
    }
}

impl AppState {
    /// Create the state with the resolved settings path and the dictation worker handle.
    pub fn new(
        settings_path: PathBuf,
        overlay_enabled: bool,
        hotkey_mode: HotkeyMode,
        worker: crate::worker::WorkerHandle,
    ) -> Self {
        Self {
            settings_path,
            overlay_enabled: AtomicBool::new(overlay_enabled),
            mic_level: Mutex::new(None),
            worker,
            installs: Mutex::new(std::collections::HashMap::new()),
            cue: Mutex::new(CueSettings::default()),
            capture_device: Mutex::new(None),
            recording: Mutex::new(RecordingState::Idle),
            generation: AtomicU64::new(0),
            shortcut: Mutex::new(None),
            shortcut_registered: AtomicBool::new(false),
            hotkey_mode: Mutex::new(hotkey_mode),
        }
    }

    /// The dictation shortcut currently registered, if any.
    pub fn shortcut(&self) -> Option<Shortcut> {
        *self.shortcut.lock()
    }

    /// Record the shortcut the handler should answer to, and whether the OS actually took it.
    pub fn set_shortcut(&self, shortcut: Option<Shortcut>, registered: bool) {
        *self.shortcut.lock() = shortcut;
        self.shortcut_registered.store(registered, Ordering::Relaxed);
    }

    /// Whether the recorded shortcut is really held by the OS.
    pub fn shortcut_registered(&self) -> bool {
        self.shortcut_registered.load(Ordering::Relaxed)
    }

    /// The active hotkey behaviour.
    pub fn hotkey_mode(&self) -> HotkeyMode {
        *self.hotkey_mode.lock()
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
        self.play_cue_for(next);
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
        self.play_cue_for(next);
        Some(generation)
    }

    /// Mirror everything this process keeps in memory from `settings.json`, in one place.
    ///
    /// Every field here exists because reading the disk at the moment it is needed would be wrong:
    /// a cue plays on a state transition, the microphone opens between the hotkey and the first
    /// word, and the hotkey handler decides hold-vs-tap inside the key event itself.
    ///
    /// One method rather than several deliberately. `hotkey.mode` used to be mirrored inside
    /// `reregister_shortcut`, which returns early when the key binding has not changed -- so
    /// switching push-to-talk to toggle, or either to hands-free, without also changing the keys
    /// did nothing until the app was restarted. The mode is not part of the binding, and nothing
    /// about the binding should decide whether it is applied.
    pub fn apply_settings(&self, settings: &Settings) {
        self.overlay_enabled
            .store(settings.overlay_enabled, Ordering::Relaxed);
        *self.cue.lock() = CueSettings::from(settings);
        let device = settings.audio.input_device.trim();
        *self.capture_device.lock() = (!device.is_empty()).then(|| device.to_string());
        *self.hotkey_mode.lock() = settings.hotkey.mode;
    }

    /// The input device to capture from, or `None` for the system default.
    pub fn capture_device(&self) -> Option<String> {
        self.capture_device.lock().clone()
    }

    /// The cue settings currently in force.
    pub fn cue_settings(&self) -> CueSettings {
        *self.cue.lock()
    }

    /// Play the cue this transition calls for, if any.
    ///
    /// Only two of the five states make a sound, and they are the two the user can act on:
    /// `Listening` means "speak now" and `Processing` means "I stopped listening". `Done` and
    /// `Error` are deliberately silent — by then the text has already appeared (or not), which is
    /// feedback enough, and a cue on every utterance's end would double the noise.
    fn play_cue_for(&self, state: RecordingState) {
        let cue = match state {
            RecordingState::Listening => Cue::Start,
            RecordingState::Processing => Cue::Stop,
            _ => return,
        };
        let s = self.cue_settings();
        if !s.enabled {
            return;
        }
        lw_platform::play_cue(s.theme, cue, s.volume);
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

#[cfg(test)]
mod tests {
    use super::*;
    use lw_core::settings::HotkeyMode;

    fn state() -> AppState {
        AppState::new(
            PathBuf::from("settings.json"),
            true,
            HotkeyMode::Toggle,
            crate::worker::WorkerHandle::detached(),
        )
    }

    /// The bug this method exists to prevent: the mode was applied inside `reregister_shortcut`,
    /// which returns early when the keys have not changed. Switching Toggle to Push-to-talk (or
    /// to Hands-free) on the same binding therefore did nothing at all until the next restart --
    /// the user picked a mode, saved, and the app kept the old behaviour with no sign of it.
    #[test]
    fn changing_only_the_mode_still_applies_it() {
        let state = state();
        assert_eq!(state.hotkey_mode(), HotkeyMode::Toggle);

        for mode in [HotkeyMode::PushToTalk, HotkeyMode::HandsFree, HotkeyMode::Toggle] {
            let settings = Settings {
                hotkey: lw_core::settings::HotkeyConfig {
                    mode,
                    ..Settings::default().hotkey
                },
                ..Settings::default()
            };
            // Same keys as whatever came before -- exactly the case that used to be skipped.
            state.apply_settings(&settings);
            assert_eq!(state.hotkey_mode(), mode, "mode {mode:?} was not applied");
        }
    }

    #[test]
    fn apply_settings_mirrors_every_field_that_is_read_off_the_hot_path() {
        let state = state();
        let settings = Settings {
            overlay_enabled: false,
            sounds_enabled: false,
            sound_volume: 0.25,
            audio: lw_core::settings::AudioConfig {
                input_device: "  Headset Microphone  ".into(),
                ..Settings::default().audio
            },
            ..Settings::default()
        };

        state.apply_settings(&settings);

        assert!(!state.overlay_enabled.load(Ordering::Relaxed));
        let cue = state.cue_settings();
        assert!(!cue.enabled);
        assert_eq!(cue.volume, 0.25);
        // Trimmed, because a stored name with stray spaces would match no device.
        assert_eq!(state.capture_device().as_deref(), Some("Headset Microphone"));
    }

    /// A blank device name means "system default", not a device called "".
    #[test]
    fn a_blank_input_device_means_the_system_default() {
        let state = state();
        let settings = Settings {
            audio: lw_core::settings::AudioConfig {
                input_device: "   ".into(),
                ..Settings::default().audio
            },
            ..Settings::default()
        };
        state.apply_settings(&settings);
        assert_eq!(state.capture_device(), None);
    }

    /// The requested binding is recorded even when the OS refuses it, so the handler still
    /// recognises its own events -- but the UI must be able to tell the two apart.
    #[test]
    fn a_refused_registration_is_recorded_as_not_registered() {
        let state = state();
        assert!(!state.shortcut_registered());
        state.set_shortcut(None, true);
        assert!(state.shortcut_registered());
        state.set_shortcut(None, false);
        assert!(!state.shortcut_registered());
    }
}
