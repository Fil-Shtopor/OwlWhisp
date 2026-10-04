//! The Settings tab.
//!
//! A port of `app/frontend/src/panels/Settings.tsx`: the hotkey, the model, the accelerator, the
//! microphone, the sound cues, the overlay and login start. Two rules carried over from the web
//! version, because both are about not lying to the user rather than about layout:
//!
//! - An accelerator the machine cannot actually use is shown as unavailable and says why. Offering
//!   a choice that silently falls back to the CPU is how "NPU" ends up meaning nothing.
//! - Login start is read back from the operating system after being set, not assumed from the
//!   checkbox, because a managed machine can refuse and a checkbox that disagreed with the OS
//!   would be worse than no checkbox.

use iced::widget::{Space, button, checkbox, column, container, pick_list, radio, row, slider};
use iced::{Element, Length, Padding};
use lw_app::accelerators::{AcceleratorAction, AcceleratorReadiness, ModelAvailability, model_availability};
use lw_core::engine::BackendPreference;
use lw_core::settings::{HotkeyMode, Settings};
use lw_core::sound::{Cue, SoundTheme};

use crate::{theme, widgets};

fn selected_model_ready(id: &str) -> bool {
    use lw_core::model::{Catalog, InstallState, entry_paths, manifests_dir};

    let Ok(catalog) = Catalog::builtin() else {
        return false;
    };
    let Some(entry) = catalog.get(id) else {
        return false;
    };
    entry_paths(
        entry,
        manifests_dir(None).as_deref(),
        &lw_app::paths::models_root(),
        lw_app::machine::probe_capabilities(),
    )
    .state
        == InstallState::Installed
}

/// A pickable wrapper, so a list can show a label while carrying the value.
macro_rules! pickable {
    ($name:ident, $inner:ty) => {
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name {
            value: $inner,
            label: String,
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.label)
            }
        }
    };
}

pickable!(BackendChoice, BackendPreference);
pickable!(ThemeChoice, SoundTheme);
pickable!(ModeChoice, HotkeyMode);
pickable!(TriggerChoice, String);
pickable!(MemoryChoice, u32);

/// What the shell knows about the microphone, mirrored here for drawing.
#[derive(Default)]
struct Mic {
    /// RMS mapped to 0..1 on a -60..0 dBFS scale, so ordinary speech sits mid-bar.
    level: f32,
    /// The loudest level since the test was switched on. Zero means genuinely no signal.
    peak: f32,
    /// Whether the test stream is open, as the worker reported it.
    open: bool,
    /// Whether dictation has the microphone instead.
    dictating: bool,
    error: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DeviceChoice(String);

impl std::fmt::Display for DeviceChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() {
            "System default"
        } else {
            &self.0
        })
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    BackendSelected(BackendPreference),
    IdleTimeoutSelected(u32),
    OpenProviderSetup(&'static str),
    ShowProviderInfo(&'static str),
    InstallRuntime(&'static str),
    RuntimeInstallTick,
    CancelRuntimeInstall,
    InstallDriver(&'static str),
    DriverFinished(std::sync::Arc<Result<String, String>>),
    InstallGpuEncoder(&'static str),
    GpuInstallTick,
    RefreshAccelerators,
    DeviceSelected(String),
    ModeSelected(HotkeyMode),
    TriggerSelected(String),
    ModifierToggled(&'static str, bool),
    /// Start or abandon "press the combination you want".
    CaptureToggled,
    /// Look at what the keyboard grab has produced, while one is open.
    CapturePoll,
    /// Open or close the microphone-test stream.
    MicTestToggled(bool),
    /// The window got wider or narrower; the layout has a breakpoint.
    Resized(f32),
    /// Speech-onset threshold for hands-free endpointing.
    VadThreshold(f32),
    /// A combination arrived while capturing.
    Captured {
        modifiers: Vec<String>,
        trigger: String,
    },
    OverlayToggled(bool),
    SoundsToggled(bool),
    ThemeSelected(SoundTheme),
    VolumeChanged(f32),
    /// Play one of the two cues, so both can be heard before either is bound to anything.
    PreviewSound(Cue),
    AutostartToggled(bool),
    StartMinimizedToggled(bool),
    Save,
}

/// How long the editor listens before giving the keyboard back.
///
/// A hair under the grab's own window, so the panel is the one that says so: the grab releasing
/// first would leave the button claiming to be listening when nothing was. Short, because while
/// it is open no other application receives a key -- see `CAPTURE_WINDOW_MS` in `lw-platform`.
const CAPTURE_WINDOW: std::time::Duration = std::time::Duration::from_secs(6);

pub struct State {
    settings: Settings,
    selected_model_ready: bool,
    /// What is on disk, to tell whether anything is unsaved.
    saved: Settings,
    devices: Vec<DeviceChoice>,
    backends: Vec<BackendChoice>,
    themes: Vec<ThemeChoice>,
    modes: Vec<ModeChoice>,
    /// Every trigger key the editor offers, in the order it offers them.
    triggers: Vec<TriggerChoice>,
    /// Whether the next keystroke should be read as a new binding rather than typed.
    capturing: bool,
    /// The keyboard grab, while one is open.
    ///
    /// `None` while capturing means this OS has no grab to offer and the window's own key events
    /// are being read instead -- which is what the editor did everywhere before, and what it still
    /// does off Windows.
    capture: Option<Box<dyn lw_platform::HotkeyCapture>>,
    /// When an open capture gives up. The grab hands the keyboard back by itself after a while --
    /// see `Platform::hotkey_capture` -- and a button still reading "Cancel capture" after it had
    /// would be inviting the user to press keys nothing was listening for.
    capture_until: Option<std::time::Instant>,
    /// The width the panel is drawn at, pushed in by the shell from window resize events.
    /// `responsive` cannot supply it inside a scrollable.
    width: f32,
    /// What the hotkey pump says about the binding it registered. Pushed in by the shell, which
    /// owns the pump; this panel cannot ask the OS itself and must not guess.
    hotkey_status: lw_app::hotkey::Status,
    /// The live input level, the test stream's state and any refusal -- all pushed in by the
    /// shell, because the worker belongs to the Dictate panel.
    mic: Mic,
    /// Which accelerators this machine can really use, by preference value.
    usable: std::collections::BTreeMap<String, bool>,
    accelerator_hardware: std::collections::BTreeMap<String, bool>,
    accelerator_provider_present: std::collections::BTreeMap<String, bool>,
    accelerator_provider_registered: std::collections::BTreeMap<String, bool>,
    accelerator_details: std::collections::BTreeMap<String, String>,
    /// Present when ONNX Runtime could not be loaded at all. In that case a missing accelerator
    /// row means "not checked", never "your hardware is absent".
    accelerator_probe_error: Option<String>,
    runtime_actions: std::collections::BTreeMap<String, lw_app::runtime_install::SetupAction>,
    runtime_install: Option<lw_app::runtime_install::Handle>,
    runtime_target: Option<&'static str>,
    provider_info: Option<&'static str>,
    installing_driver: Option<&'static str>,
    driver_install_failed: std::collections::BTreeSet<&'static str>,
    gpu_install: Option<lw_app::install::Handle>,
    gpu_target: Option<&'static str>,
    gpu_check_failed: Option<&'static str>,
    notice: Option<String>,
    error: Option<String>,
}

impl State {
    pub fn new() -> Self {
        let path = lw_app::paths::settings_path();
        let settings = Settings::load(&path).unwrap_or_default();
        let selected_model_ready = selected_model_ready(&settings.model_id);
        let diag = lw_app::diagnostics::collect(env!("CARGO_PKG_VERSION"));
        let runtime_actions = diag
            .accelerators
            .iter()
            .filter_map(|a| a.setup_action.map(|action| (a.id.to_string(), action)))
            .collect();
        let usable = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.usable))
            .collect();
        let accelerator_details = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.detail.clone()))
            .collect();
        let accelerator_hardware = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.hardware_present))
            .collect();
        let accelerator_provider_present = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.present))
            .collect();
        let accelerator_provider_registered = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.registered))
            .collect();
        let accelerator_probe_error = diag.accelerators_error.or(diag.runtime_error);
        let backends = BackendPreference::all()
            .into_iter()
            .map(|p| BackendChoice {
                value: p,
                label: p.label().to_string(),
            })
            .collect();

        Self {
            saved: settings.clone(),
            settings,
            selected_model_ready,
            devices: std::iter::once(DeviceChoice(String::new()))
                .chain(
                    lw_platform::audio::list_input_devices()
                        .into_iter()
                        .map(DeviceChoice),
                )
                .collect(),
            backends,
            themes: lw_core::sound::ALL_SOUND_THEMES
                .iter()
                .map(|t| ThemeChoice {
                    value: *t,
                    label: t.label().to_string(),
                })
                .collect(),
            modes: [
                (HotkeyMode::PushToTalk, "Push to talk - hold, release to stop"),
                (HotkeyMode::Toggle, "Toggle - tap to start, tap to stop"),
                (
                    HotkeyMode::HandsFree,
                    "Hands free - tap to start, silence ends it",
                ),
            ]
            .into_iter()
            .map(|(value, label)| ModeChoice {
                value,
                label: label.to_string(),
            })
            .collect(),
            triggers: trigger_choices(),
            capturing: false,
            capture: None,
            capture_until: None,
            width: 1000.0,
            hotkey_status: lw_app::hotkey::Status::default(),
            mic: Mic::default(),
            usable,
            accelerator_hardware,
            accelerator_provider_present,
            accelerator_provider_registered,
            accelerator_details,
            accelerator_probe_error,
            runtime_actions,
            runtime_install: None,
            runtime_target: None,
            provider_info: None,
            installing_driver: None,
            driver_install_failed: Default::default(),
            gpu_install: None,
            gpu_target: None,
            gpu_check_failed: None,
            notice: None,
            error: None,
        }
    }

    /// The saved startup preference, used before the main window is created.
    pub fn start_minimized(&self) -> bool {
        self.saved.start_minimized
    }

    /// The shell hands this over after every poll; the panel only displays it.
    pub fn set_hotkey_status(&mut self, status: lw_app::hotkey::Status) {
        self.hotkey_status = status;
    }

    /// Likewise for the microphone. `dictating` means dictation has it, not this test.
    pub fn set_mic(&mut self, level: f32, open: bool, dictating: bool, error: Option<&str>) {
        self.mic.level = level;
        self.mic.open = open;
        self.mic.dictating = dictating;
        self.mic.error = error.map(str::to_string);
        if open && level > self.mic.peak {
            self.mic.peak = level;
        }
        if !open {
            self.mic.peak = 0.0;
        }
    }

    /// Whether the microphone test is on, so the shell can close it when this tab goes away.
    pub fn mic_test_on(&self) -> bool {
        self.mic.open
    }

    pub fn busy(&self) -> bool {
        self.runtime_install.is_some()
            || self.gpu_install.is_some()
            || self.installing_driver.is_some()
            || self.capturing
    }

    fn refresh_accelerators(&mut self) {
        let diag = lw_app::diagnostics::collect(env!("CARGO_PKG_VERSION"));
        self.runtime_actions = diag
            .accelerators
            .iter()
            .filter_map(|a| a.setup_action.map(|action| (a.id.to_string(), action)))
            .collect();
        self.usable = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.usable))
            .collect();
        self.accelerator_hardware = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.hardware_present))
            .collect();
        self.accelerator_provider_present = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.present))
            .collect();
        self.accelerator_provider_registered = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.registered))
            .collect();
        self.accelerator_details = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.detail.clone()))
            .collect();
        self.accelerator_probe_error = diag.accelerators_error.or(diag.runtime_error);
        self.refresh_model_readiness();
    }

    pub fn refresh_model_readiness(&mut self) {
        self.selected_model_ready = selected_model_ready(&self.settings.model_id);
    }

    /// Stop listening, whichever way the listening was being done.
    pub fn stop_capture(&mut self) {
        if let Some(grab) = self.capture.take() {
            let (hook, grabbed) = grab.key_counts();
            tracing::info!(
                hook_keys = hook,
                grabbed_keys = grabbed,
                "shortcut editor has given the keyboard back",
            );
        }
        self.capture_until = None;
        self.capturing = false;
    }

    /// Listen for the next keystroke, but only while the user asked to be listened to.
    ///
    /// Not a permanent listener: this window has ordinary text fields in it, and a panel that read
    /// every keypress as a hotkey would rebind the shortcut while someone typed a model name.
    ///
    /// **Both** listeners run at once, and that is the point. The keyboard grab reads what a
    /// window never gets offered -- anything with Meta in it, and two modifiers held together --
    /// but it depends on a system-wide keyboard hook, and a hook is a thing an operating system
    /// can take away without telling anyone. The window's own key events depend on nothing and
    /// read ordinary combinations perfectly well. Running only the grab made the editor's ability
    /// to read *any* key hostage to the hook still being there; running only the window made Meta
    /// unbindable. Whichever answers first wins, and neither can leave the editor deaf.
    pub fn subscription(&self) -> iced::Subscription<Message> {
        let mut subscriptions = Vec::new();
        if self.runtime_install.is_some() {
            subscriptions.push(
                iced::time::every(std::time::Duration::from_millis(200)).map(|_| Message::RuntimeInstallTick),
            );
        }
        if self.gpu_install.is_some() {
            subscriptions.push(
                iced::time::every(std::time::Duration::from_millis(200)).map(|_| Message::GpuInstallTick),
            );
        }
        if self.capturing {
            let window_keys = Self::window_key_capture();
            if self.capture.is_some() {
                subscriptions.push(
                    iced::time::every(std::time::Duration::from_millis(25)).map(|_| Message::CapturePoll),
                );
            }
            subscriptions.push(window_keys);
        }
        iced::Subscription::batch(subscriptions)
    }

    /// The window's own key events, read as a binding.
    fn window_key_capture() -> iced::Subscription<Message> {
        // `on_key_press` is gone in iced 0.14; `listen` reports every keyboard event and the
        // press is picked out here. Same events, one more line.
        iced::keyboard::listen().filter_map(|event| {
            let iced::keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
                return None;
            };
            // Esc on its own cancels, exactly as it did in the web editor. Esc *with* a modifier is
            // a perfectly good trigger key and is taken as one.
            if key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) && modifiers.is_empty() {
                return Some(Message::CaptureToggled);
            }
            // Still holding modifiers down: wait for the real key rather than binding half a combo.
            let trigger = trigger_from_key(&key)?;
            let mut names = Vec::new();
            if modifiers.control() {
                names.push("ctrl".to_string());
            }
            if modifiers.alt() {
                names.push("alt".to_string());
            }
            if modifiers.shift() {
                names.push("shift".to_string());
            }
            if modifiers.logo() {
                names.push("meta".to_string());
            }
            Some(Message::Captured {
                modifiers: names,
                trigger,
            })
        })
    }

    fn dirty(&self) -> bool {
        self.settings != self.saved
    }

    /// Apply one message; the answer is whether `settings.json` was written.
    ///
    /// The shell needs to know, because things outside this panel run off that file: the dictation
    /// worker holds its own copy, and the global hotkey is registered with the OS. Neither will
    /// notice a new file on its own, and neither should be made to poll for one.
    pub fn update(&mut self, message: Message) -> bool {
        let mut wrote = false;
        self.notice = None;
        match message {
            Message::BackendSelected(b) => self.settings.backend = b,
            Message::IdleTimeoutSelected(v) => self.settings.model_idle_timeout_secs = v,
            Message::OpenProviderSetup(_) => {}
            Message::ShowProviderInfo(id) => {
                self.provider_info = (self.provider_info != Some(id)).then_some(id);
            }
            Message::InstallRuntime(id) => {
                if self.runtime_install.is_none()
                    && let Some(accel) = lw_core::capabilities::Accelerator::from_id(id)
                {
                    self.runtime_target = Some(id);
                    self.error = None;
                    self.runtime_install = Some(lw_app::runtime_install::start(accel));
                }
            }
            Message::CancelRuntimeInstall => {
                if let Some(handle) = &self.runtime_install {
                    handle.cancel();
                }
            }
            Message::RuntimeInstallTick => {
                if let Some(result) = self.runtime_install.as_ref().and_then(|h| h.poll()) {
                    self.runtime_install = None;
                    let target = self.runtime_target.take();
                    self.refresh_accelerators();
                    match result {
                        Ok(_) => {
                            self.notice = Some(
                                "Runtime installed and accelerator checked. Preparing the GPU model..."
                                    .into(),
                            );
                            if let Some(id) = target {
                                wrote |= self.update(Message::InstallGpuEncoder(id));
                            }
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
            }
            Message::InstallDriver(id) => self.installing_driver = Some(id),
            Message::InstallGpuEncoder(id) => {
                if self.gpu_install.is_none()
                    && lw_core::capabilities::Accelerator::from_id(id)
                        .is_some_and(|a| a.kind() == lw_core::capabilities::AcceleratorKind::Gpu)
                {
                    self.gpu_target = Some(id);
                    self.gpu_check_failed = None;
                    self.gpu_install = Some(lw_app::install::start_gpu_encoder(
                        &lw_app::paths::settings_path(),
                        lw_core::capabilities::Accelerator::from_id(id).expect("known GPU provider"),
                    ));
                }
            }
            Message::GpuInstallTick => {
                if let Some(outcome) = self.gpu_install.as_ref().and_then(|h| h.poll()) {
                    self.gpu_install = None;
                    let target = self.gpu_target.take();
                    match outcome {
                        lw_app::install::Progress::Done { .. } => {
                            self.gpu_check_failed = None;
                            if let Some(accel) = target.and_then(lw_core::capabilities::Accelerator::from_id)
                            {
                                self.settings.backend = BackendPreference::for_accelerator(accel);
                                match self.settings.save(&lw_app::paths::settings_path()) {
                                    Ok(()) => {
                                        self.saved = self.settings.clone();
                                        self.notice =
                                            Some(format!("GPU model installed; {} selected.", accel.label()));
                                        self.error = None;
                                        wrote = true;
                                    }
                                    Err(e) => {
                                        self.error = Some(format!(
                                            "GPU model installed, but accelerator selection was not saved: {e}"
                                        ))
                                    }
                                }
                            }
                        }
                        lw_app::install::Progress::Failed { message } => {
                            self.gpu_check_failed = target;
                            self.error = Some(message);
                        }
                        lw_app::install::Progress::Cancelled => {
                            self.notice = Some("GPU model download stopped; click again to resume.".into())
                        }
                        _ => {}
                    }
                }
            }
            Message::DriverFinished(result) => {
                let attempted = self.installing_driver.take();
                self.refresh_accelerators();
                if let Some(id) = attempted {
                    if result.is_err() || !self.usable.get(id).copied().unwrap_or(false) {
                        self.driver_install_failed.insert(id);
                    } else {
                        self.driver_install_failed.remove(id);
                    }
                }
                match result.as_ref() {
                    Ok(message) => {
                        self.error = None;
                        self.notice = Some(message.clone());
                    }
                    Err(message) => self.error = Some(message.clone()),
                }
            }
            Message::RefreshAccelerators => self.refresh_accelerators(),
            Message::DeviceSelected(d) => self.settings.audio.input_device = d,
            Message::ModeSelected(m) => self.settings.hotkey.mode = m,
            Message::TriggerSelected(t) => self.settings.hotkey.trigger = t,
            Message::ModifierToggled(id, on) => {
                // Rebuilt in a stable canonical order, dropping any alias spelling an older file
                // may hold: ticking a box loaded as "control" must not leave both in the list.
                self.settings.hotkey.modifiers = lw_app::hotkey::MODIFIERS
                    .iter()
                    .filter(|m| {
                        if m.id == id {
                            on
                        } else {
                            lw_app::hotkey::has_modifier(&self.settings.hotkey, m.id)
                        }
                    })
                    .map(|m| m.id.to_string())
                    .collect();
            }
            Message::CaptureToggled => {
                if self.capturing {
                    self.stop_capture();
                } else {
                    // The grab is preferred and its absence is not an error worth showing: the
                    // window's own key events still capture most combinations, just not the ones
                    // involving the Windows key. Logged, so a machine where the hook failed to
                    // install can be told apart from one that never had a hook.
                    match lw_platform::platform().hotkey_capture() {
                        Ok(grab) => {
                            // At info, not debug. This is a system-wide keyboard grab: while it
                            // is open no other application receives a key, and a log that cannot
                            // say when it opened and closed is no use at all when someone reports
                            // that their keyboard stopped working.
                            let (hook, grabbed) = grab.key_counts();
                            tracing::info!(
                                hook_keys = hook,
                                grabbed_keys = grabbed,
                                "shortcut editor has taken the keyboard",
                            );
                            self.capture = Some(grab);
                        }
                        Err(e) => {
                            tracing::info!(
                                "no keyboard grab for the shortcut editor ({e}); reading this \
                                 window's own key events instead"
                            );
                            self.capture = None;
                        }
                    }
                    self.capturing = true;
                    self.capture_until = self
                        .capture
                        .is_some()
                        .then(|| std::time::Instant::now() + CAPTURE_WINDOW);
                    // A refusal from the last attempt has been answered by trying again.
                    self.error = None;
                }
            }
            Message::CapturePoll => {
                if self.capture_until.is_some_and(|t| std::time::Instant::now() >= t) {
                    self.stop_capture();
                    self.error = Some("Nothing was pressed, so the keyboard was handed back.".into());
                    return false;
                }
                let Some(grab) = &self.capture else {
                    return false;
                };
                // One event ends the session; anything after it belongs to nobody.
                if let Ok(captured) = grab.events().try_recv() {
                    tracing::info!(?captured, "the shortcut editor read a combination");
                    match captured {
                        lw_platform::Captured::Cancelled => self.stop_capture(),
                        lw_platform::Captured::Combo(raw) => {
                            match lw_app::hotkey::captured_binding(raw) {
                                Some((modifiers, trigger)) => {
                                    self.settings.hotkey.modifiers = modifiers;
                                    self.settings.hotkey.trigger = trigger;
                                    self.stop_capture();
                                }
                                // Reported rather than ignored. A media key, or Fn on a keyboard
                                // that reports it: the user pressed something and is owed an
                                // answer, and "nothing happened" is not one.
                                None => {
                                    self.error = Some(
                                        "That key cannot be part of a shortcut. Pick a letter, \
                                         a digit, a function key, or Space, Tab, Enter or Esc."
                                            .into(),
                                    );
                                    self.stop_capture();
                                }
                            }
                        }
                    }
                }
            }
            Message::Resized(w) => self.width = w,
            Message::VadThreshold(v) => self.settings.vad.threshold = v,
            // Nothing is set here. The shell forwards this to the worker, and the answer comes
            // back through `set_mic` -- the switch shows what happened, not what was asked.
            Message::MicTestToggled(_) => {}
            Message::Captured { modifiers, trigger } => {
                self.settings.hotkey.modifiers = modifiers;
                self.settings.hotkey.trigger = trigger;
                self.stop_capture();
            }
            Message::OverlayToggled(v) => self.settings.overlay_enabled = v,
            Message::SoundsToggled(v) => self.settings.sounds_enabled = v,
            Message::ThemeSelected(t) => self.settings.sound_theme = t,
            Message::VolumeChanged(v) => self.settings.sound_volume = v,
            Message::PreviewSound(cue) => {
                lw_platform::play_cue(self.settings.sound_theme, cue, self.settings.sound_volume);
            }
            Message::StartMinimizedToggled(v) => self.settings.start_minimized = v,
            Message::AutostartToggled(v) => {
                // Written through the platform and then read back, so the checkbox can never claim
                // a registration the OS refused.
                match lw_platform::autostart::set(v) {
                    Ok(()) => match lw_platform::autostart::is_enabled() {
                        Ok(actual) => {
                            self.settings.autostart = actual;
                            if actual != v {
                                self.error =
                                    Some("The operating system refused to change login start.".into());
                            }
                        }
                        Err(e) => self.error = Some(format!("Could not read login start: {e}")),
                    },
                    Err(e) => self.error = Some(format!("Could not set login start: {e}")),
                }
            }
            Message::Save => {
                let path = lw_app::paths::settings_path();
                match self.settings.save(&path) {
                    Ok(()) => {
                        // Read back what landed rather than assume it. This is what the Reload
                        // button was for, done where it belongs: some settings are not ours to
                        // decide -- a managed machine can refuse the autostart registration, and
                        // the file is rewritten to what the OS actually did. Showing the form the
                        // user typed instead of the file that exists is how a setting appears to
                        // have been accepted when it was not.
                        self.settings = Settings::load(&path).unwrap_or_else(|_| self.settings.clone());
                        self.saved = self.settings.clone();
                        self.notice = Some("Saved.".into());
                        self.error = None;
                        wrote = true;
                    }
                    Err(e) => self.error = Some(format!("Could not save: {e}")),
                }
            }
        }
        wrote
    }

    /// Below this width the form is one column; above it, two.
    ///
    /// The same 1200 the web version used, and for the same reason: the accelerator picker's rows
    /// are the widest thing in the form and the hotkey editor's the narrowest, so they are given a
    /// 1:2 split rather than an even one -- an even split wasted space on one side and wrapped
    /// rows on the other.
    const TWO_COLUMN_AT: f32 = 1200.0;

    pub fn view(&self) -> Element<'_, Message> {
        // Deliberate groups, not auto-flow: which setting lands where must not depend on how tall
        // the accelerator card happens to be on this machine. What you adjust on the left, what
        // describes the hardware on the right.
        let adjust: Vec<Element<'_, Message>> = vec![
            self.hotkey_card(),
            self.sound_card(),
            self.overlay_card(),
            self.autostart_card(),
            self.memory_card(),
        ];
        let hardware: Vec<Element<'_, Message>> =
            vec![self.accelerator_card(), self.microphone_card(), self.vad_card()];

        let body: Element<'_, Message> = if self.width >= Self::TWO_COLUMN_AT {
            let mut left = column![].spacing(12).width(Length::FillPortion(1));
            for card in adjust {
                left = left.push(card);
            }
            let mut right = column![].spacing(12).width(Length::FillPortion(2));
            for card in hardware {
                right = right.push(card);
            }
            column![
                row![left, right].spacing(20).align_y(iced::Alignment::Start),
                // Save spans both, because it applies to both.
                self.save_row(),
                Space::new().height(8),
            ]
            .spacing(12)
            .padding(Padding::from([0, 8]))
            .into()
        } else {
            let mut one = column![].spacing(12).padding(Padding::from([0, 8]));
            for card in adjust.into_iter().chain(hardware) {
                one = one.push(card);
            }
            one.push(self.save_row()).push(Space::new().height(8)).into()
        };

        body
    }

    fn memory_card(&self) -> Element<'_, Message> {
        let mut choices: Vec<_> = [
            (0, "Keep model loaded"),
            (60, "Unload after 1 minute idle"),
            (300, "Unload after 5 minutes idle"),
            (900, "Unload after 15 minutes idle"),
            (1800, "Unload after 30 minutes idle"),
        ]
        .into_iter()
        .map(|(value, label)| MemoryChoice {
            value,
            label: label.into(),
        })
        .collect();
        let secs = self.settings.model_idle_timeout_secs;
        let selected = choices
            .iter()
            .find(|choice| choice.value == secs)
            .cloned()
            .unwrap_or_else(|| {
                let choice = MemoryChoice {
                    value: secs,
                    label: format!("Unload after {secs} seconds idle"),
                };
                choices.push(choice.clone());
                choice
            });
        widgets::card(
            column![
                widgets::heading("Background memory"),
                pick_list(choices, Some(selected), |choice: MemoryChoice| {
                    Message::IdleTimeoutSelected(choice.value)
                })
                .text_size(14)
                .width(Length::Fill),
                widgets::prose(
                    "Frees the speech model while you are not dictating. The next transcription \
                     reloads it and takes longer. Keep it loaded for faster \
                     responses; RAM usage then stays higher between dictations.",
                ),
            ]
            .spacing(8),
        )
        .into()
    }

    /// When hands-free decides you have finished speaking.
    ///
    /// The only setting here that changes what the engine hears rather than what the interface
    /// does, which is why it says what moving it costs in both directions.
    fn vad_card(&self) -> Element<'_, Message> {
        widgets::card(
            column![
                widgets::heading("Voice activity"),
                widgets::sub(
                    "Used by the hands-free mode to decide an utterance has ended. The other two \
                     hotkey modes end it when you say so, and ignore this.",
                ),
                row![
                    widgets::field_label("Threshold"),
                    slider(0.0..=1.0, self.settings.vad.threshold, Message::VadThreshold)
                        .step(0.01_f32)
                        .width(240),
                    widgets::mono(format!("{:.2}", self.settings.vad.threshold)),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
                widgets::prose(
                    "Higher needs louder, clearer speech before it counts as speech: it stops a \
                     noisy room starting a recording, and it also stops a quiet voice. Lower does \
                     the opposite.",
                ),
            ]
            .spacing(8),
        )
        .into()
    }

    /// The hotkey editor, in the order the web version settled on: what is bound, how it behaves,
    /// which keys make it up, and -- last, because it is the only line here that is not the user's
    /// own choice -- whether the operating system took it.
    fn hotkey_card(&self) -> Element<'_, Message> {
        let h = &self.settings.hotkey;

        let preview = row![
            widgets::mono(lw_app::hotkey::format_hotkey(h)),
            button(widgets::button_label(if self.capturing {
                "Cancel capture"
            } else {
                "Capture keystroke"
            }))
            .padding(Padding::from([6, 14]))
            .on_press(Message::CaptureToggled)
            .style(theme::action(false)),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        let mut card = column![widgets::heading("Hotkey"), preview].spacing(8);
        if self.capturing {
            card = card.push(widgets::sub(
                "Press the combination you want to use. Esc on its own cancels.",
            ));
            card = card.push(widgets::sub(if self.capture.is_some() {
                "The keyboard is held while this is open, so nothing you press reaches anything \
                 else -- Meta (the Win or Command key) included. Two modifiers held together and \
                 then let go are read as a binding of their own."
            } else {
                "This build cannot hold the keyboard on this operating system, so keys the \
                 window manager takes first -- most combinations involving Meta (Win/Cmd) -- \
                 will not arrive. Those can still be set with the controls below."
            }));
            card = card.push(widgets::sub(
                "Fn is the one key that cannot be bound anywhere: on almost every laptop it is \
                 handled inside the keyboard itself and never reaches the operating system, so \
                 no application ever sees it.",
            ));
        }

        // Radios rather than a dropdown: three choices, each needing a sentence of explanation,
        // and a dropdown hides two of the three behind a click.
        let mut modes = column![widgets::field_label("Mode")].spacing(4);
        for m in &self.modes {
            modes = modes.push(
                radio(m.label.clone(), m.value, Some(h.mode), Message::ModeSelected)
                    .size(15)
                    .text_size(14),
            );
        }
        card = card.push(modes);

        let mut boxes = row![].spacing(14);
        for m in &lw_app::hotkey::MODIFIERS {
            let label = match m.detail {
                Some(d) => format!("{} ({d})", m.label),
                None => m.label.to_string(),
            };
            boxes = boxes.push(
                checkbox(lw_app::hotkey::has_modifier(h, m.id))
                    .label(label)
                    .on_toggle(move |v| Message::ModifierToggled(m.id, v))
                    .size(16)
                    .text_size(14),
            );
        }
        card = card.push(column![widgets::field_label("Modifiers"), boxes].spacing(4));

        // A binding written elsewhere can hold a key this editor never offers. It goes at the top
        // of the list rather than being dropped: a settings file should read back as what it says.
        let saved = lw_app::hotkey::trigger_of(h);
        let mut triggers = self.triggers.clone();
        if !saved.is_empty() && !triggers.iter().any(|t| t.value == saved) {
            triggers.insert(
                0,
                TriggerChoice {
                    label: format!("{} (saved)", lw_app::hotkey::trigger_label(&saved)),
                    value: saved.clone(),
                },
            );
        }
        let selected = triggers.iter().find(|t| t.value == saved).cloned();
        card = card.push(
            column![
                widgets::field_label("Trigger key"),
                pick_list(triggers, selected, |t: TriggerChoice| {
                    Message::TriggerSelected(t.value)
                })
                .placeholder("Pick one")
                .text_size(14)
                .width(220),
                widgets::sub(
                    "With no key, two modifiers held together are the shortcut -- Ctrl+Meta, \
                     say. The window manager will not register that, so this app watches the \
                     keyboard itself for it. One modifier alone is refused: it would fire every \
                     time you pressed it for anything else.",
                ),
            ]
            .spacing(4),
        );

        if let Err(e) = h.validate() {
            card = card.push(iced::widget::text(e.to_string()).size(13).color(theme::BAD));
        }

        card = card.push(
            column![
                widgets::field_label("Registered with the OS"),
                self.registration(),
            ]
            .spacing(4),
        );

        widgets::card(card).into()
    }

    /// What the OS actually holds, which is not the same question as what the settings say.
    ///
    /// The two disagree in the case that matters: a combination another application already owns
    /// is saved happily and registers as nothing, and the only symptom is a hotkey that does
    /// nothing at all. This line is where that becomes visible.
    fn registration(&self) -> Element<'_, Message> {
        match (&self.hotkey_status.bound, &self.hotkey_status.error) {
            (_, Some(e)) => column![
                widgets::sub(
                    "The OS holds no accelerator for this app right now. If you just saved, \
                     registration did not take - another app may already own the combination.",
                ),
                iced::widget::text(e.clone()).size(13).color(theme::BAD),
            ]
            .spacing(2)
            .into(),
            (Some(spec), None) => {
                // A binding with no key is never handed to the window manager -- it refuses them
                // -- so saying the OS holds it would be false. This app is watching the keyboard
                // for it instead, which is a different guarantee and worth naming as one.
                let by_hook = lw_app::hotkey::trigger_of_spec(spec).is_empty();
                row![
                    widgets::mono(
                        spec.modifiers
                            .iter()
                            .map(|m| lw_app::hotkey::modifier_label(m))
                            .chain((!by_hook).then(|| lw_app::hotkey::trigger_label(&spec.trigger)))
                            .collect::<Vec<_>>()
                            .join(" + "),
                    ),
                    widgets::sub(if by_hook {
                        "is watched for on the keyboard directly. The window manager does not \
                         register combinations without a key, so this one does not depend on it."
                    } else {
                        "is the combination the OS currently holds."
                    }),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .into()
            }
            (None, None) => widgets::sub("Checking...").into(),
        }
    }

    fn accelerator_card(&self) -> Element<'_, Message> {
        use lw_core::capabilities::{ALL_ACCELERATORS, Accelerator as A, AcceleratorKind};

        let model_dir = lw_app::paths::models_root().join(&self.settings.model_id);
        let parakeet = self.settings.model_id == "parakeet-tdt-0.6b-v3";
        let model_ready = self.selected_model_ready;
        let gpu_model_ready = model_dir.join("encoder-model.onnx").exists()
            && model_dir.join("encoder-model.onnx.data").exists();
        let npu_model_ready = model_dir.join("encoder-static-t2000.onnx").is_file()
            || model_dir.join("encoder-static.onnx").is_file()
            || model_dir.join("encoder-model.onnx").is_file();
        let can_activate = |a: A| {
            model_availability(
                a,
                &AcceleratorReadiness {
                    parakeet,
                    model_ready,
                    gpu_model_ready,
                    npu_model_ready,
                    probe_ok: self.accelerator_probe_error.is_none(),
                    hardware_present: self.accelerator_hardware.get(a.id()).copied().unwrap_or(false),
                    runtime_installable: matches!(
                        self.runtime_actions.get(a.id()),
                        Some(lw_app::runtime_install::SetupAction::DownloadRuntime)
                    ),
                    driver_needed: matches!(
                        self.runtime_actions.get(a.id()),
                        Some(lw_app::runtime_install::SetupAction::InstallDriver)
                    ),
                    provider_registered: self
                        .accelerator_provider_registered
                        .get(a.id())
                        .copied()
                        .unwrap_or(false),
                    usable: self.usable.get(a.id()).copied().unwrap_or(false),
                    gpu_check_failed: self.gpu_check_failed == Some(a.id())
                        || self.gpu_target == Some(a.id()),
                    driver_install_failed: self.driver_install_failed.contains(a.id()),
                },
            )
            .0 == ModelAvailability::Available
        };
        let choices: Vec<_> = self
            .backends
            .iter()
            .filter(|b| {
                if b.value == self.settings.backend || b.value == BackendPreference::Automatic {
                    return true;
                }
                match b.value.accelerator() {
                    Some(A::Cpu) => true,
                    Some(a) => can_activate(a),
                    None => {
                        let kind = if b.value == BackendPreference::ForceNpu {
                            AcceleratorKind::Npu
                        } else {
                            AcceleratorKind::Gpu
                        };
                        ALL_ACCELERATORS
                            .iter()
                            .any(|a| a.kind() == kind && can_activate(*a))
                    }
                }
            })
            .cloned()
            .collect();
        let selected = self
            .backends
            .iter()
            .find(|b| b.value == self.settings.backend)
            .cloned();
        let saved_choice_visible = selected.is_some();

        let mut body = column![
            widgets::heading("Accelerator"),
            widgets::sub("For this model shows what this build can run now. Info distinguishes detected hardware from a missing provider. Automatic prefers NPU, then CPU, then GPU.").width(Length::Fill),
            pick_list(choices, selected.clone(), |b: BackendChoice| {
                Message::BackendSelected(b.value)
            })
            .text_size(14)
            .width(Length::Fill),
        ]
        .spacing(8)
        .width(Length::Fill);

        if !saved_choice_visible {
            body = body.push(widgets::prose(format!(
                "The saved accelerator preference '{}' does not match hardware detected on this machine. Choose another option to continue.",
                self.settings.backend.label()
            )));
        }

        // The hardware card receives two thirds of a two-column window, not the whole window.
        let card_width = if self.width >= Self::TWO_COLUMN_AT {
            (self.width - 36.0) * 2.0 / 3.0
        } else {
            self.width - 16.0
        };
        let compact = card_width < 680.0;
        if !compact {
            body = body.push(
                row![
                    container(widgets::field_label("Accelerator")).width(Length::Fixed(190.0)),
                    container(widgets::field_label("For this model")).width(Length::Fixed(155.0)),
                    container(widgets::field_label("Action")).width(Length::Fill),
                    widgets::field_label("Info"),
                ]
                .spacing(8),
            );
        }

        // Keep the full supported matrix visible so an unavailable provider has a clear reason.
        for b in &self.backends {
            let Some(accel) = b.value.accelerator() else {
                continue;
            };
            let usable = self.usable.get(accel.id()).copied().unwrap_or(false);
            let hardware_present = self
                .accelerator_hardware
                .get(accel.id())
                .copied()
                .unwrap_or(false);
            let detail = self.accelerator_details.get(accel.id());
            let provider_present = self
                .accelerator_provider_present
                .get(accel.id())
                .copied()
                .unwrap_or(false);
            let provider_registered = self
                .accelerator_provider_registered
                .get(accel.id())
                .copied()
                .unwrap_or(false);
            let (availability, needed_action) = model_availability(
                accel,
                &AcceleratorReadiness {
                    parakeet,
                    model_ready,
                    gpu_model_ready,
                    npu_model_ready,
                    probe_ok: self.accelerator_probe_error.is_none(),
                    hardware_present,
                    provider_registered,
                    runtime_installable: matches!(
                        self.runtime_actions.get(accel.id()),
                        Some(lw_app::runtime_install::SetupAction::DownloadRuntime)
                    ),
                    driver_needed: matches!(
                        self.runtime_actions.get(accel.id()),
                        Some(lw_app::runtime_install::SetupAction::InstallDriver)
                    ),
                    usable,
                    gpu_check_failed: self.gpu_check_failed == Some(accel.id())
                        || self.gpu_target == Some(accel.id()),
                    driver_install_failed: self.driver_install_failed.contains(accel.id()),
                },
            );
            let status: Element<'_, Message> = match availability {
                ModelAvailability::Available => widgets::badge_yes(availability.label()),
                ModelAvailability::Unavailable => widgets::badge_no(availability.label()),
                ModelAvailability::NeedAdditionalAction => {
                    widgets::badge(availability.label(), theme::ESTIMATE)
                }
            };
            let action: Element<'_, Message> = if needed_action == Some(AcceleratorAction::DownloadGpuModel) {
                let label = if let Some(handle) = &self.gpu_install {
                    if self.gpu_target == Some(accel.id()) {
                        let state = handle.state();
                        if state.finishing {
                            "Checking GPU...".to_string()
                        } else {
                            match state.fraction() {
                                Some(fraction) => format!("Downloading {:.0}%", fraction * 100.0),
                                None => "Downloading...".to_string(),
                            }
                        }
                    } else {
                        "Download in progress".to_string()
                    }
                } else {
                    if gpu_model_ready {
                        "Retry GPU check"
                    } else {
                        "Download GPU model"
                    }
                    .to_string()
                };
                button(widgets::button_label(label))
                    .padding(Padding::from([5, 10]))
                    .style(theme::action(false))
                    .on_press_maybe(
                        self.gpu_install
                            .is_none()
                            .then_some(Message::InstallGpuEncoder(accel.id())),
                    )
                    .into()
            } else if needed_action == Some(AcceleratorAction::DownloadRuntime) {
                let active = self.runtime_target == Some(accel.id());
                let label = if active {
                    self.runtime_install
                        .as_ref()
                        .map(|h| {
                            let progress = h.progress();
                            match progress.fraction {
                                Some(f) => format!("Downloading {:.0}%", f * 100.0),
                                None => progress.label,
                            }
                        })
                        .unwrap_or_else(|| "Download runtime".into())
                } else {
                    "Download runtime".into()
                };
                let download = button(widgets::button_label(label))
                    .padding(Padding::from([5, 10]))
                    .style(theme::action(false))
                    .on_press_maybe(
                        (self.runtime_install.is_none()
                            && self.gpu_install.is_none()
                            && self.installing_driver.is_none())
                        .then_some(Message::InstallRuntime(accel.id())),
                    );
                if active {
                    row![
                        download,
                        button(widgets::button_label("Cancel"))
                            .on_press(Message::CancelRuntimeInstall)
                            .padding(Padding::from([5, 10]))
                            .style(theme::action(false))
                    ]
                    .spacing(4)
                    .into()
                } else {
                    download.into()
                }
            } else if needed_action == Some(AcceleratorAction::InstallDriver) {
                button(widgets::button_label(
                    if self.installing_driver == Some(accel.id()) {
                        "Installing..."
                    } else {
                        "Install driver"
                    },
                ))
                .padding(Padding::from([5, 10]))
                .style(theme::action(false))
                .on_press_maybe(
                    self.installing_driver
                        .is_none()
                        .then_some(Message::InstallDriver(accel.id())),
                )
                .into()
            } else {
                widgets::sub(if availability == ModelAvailability::Available {
                    "Use now"
                } else if !hardware_present {
                    "Hardware not found on this PC"
                } else if !model_ready {
                    "Install model first"
                } else if !parakeet && accel != A::Cpu {
                    "CPU-only model"
                } else if self.gpu_check_failed == Some(accel.id()) {
                    "GPU check failed"
                } else if self.driver_install_failed.contains(accel.id()) {
                    "Driver did not enable it"
                } else if accel == A::QnnNpu && !npu_model_ready {
                    "NPU model unavailable"
                } else if accel.kind() == AcceleratorKind::Npu && accel != A::QnnNpu {
                    "No model artifact"
                } else if !provider_present {
                    "No compatible runtime package"
                } else if !provider_registered {
                    "Provider could not load"
                } else {
                    "See details"
                })
                .into()
            };
            let info = button(widgets::button_label("?"))
                .padding(Padding::from([5, 10]))
                .style(theme::action(false))
                .on_press(Message::ShowProviderInfo(accel.id()));
            if compact {
                body = body.push(widgets::inset(
                    column![
                        row![widgets::sub(&b.label), status, info]
                            .spacing(8)
                            .align_y(iced::Alignment::Center),
                        action,
                    ]
                    .spacing(6),
                ));
            } else {
                body = body.push(
                    row![
                        container(widgets::sub(&b.label)).width(Length::Fixed(190.0)),
                        container(status).width(Length::Fixed(155.0)),
                        container(action).width(Length::Fill),
                        info,
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                );
            }
            if self.provider_info == Some(accel.id()) {
                body = body.push(widgets::prose(format!(
                    "{}: {}",
                    b.label,
                    detail.map(String::as_str).unwrap_or("not checked")
                )));
                if let Some((description, url)) = provider_guide(accel.id()) {
                    body = body.push(widgets::prose(description));
                    body = body.push(
                        button(widgets::button_label("Documentation ↗"))
                            .padding(Padding::from([5, 10]))
                            .style(theme::action(false))
                            .on_press(Message::OpenProviderSetup(url)),
                    );
                }
            }
        }
        body = body.push(
            button(widgets::button_label("Check again"))
                .padding(Padding::from([5, 10]))
                .style(theme::action(false))
                .on_press(Message::RefreshAccelerators),
        );
        body = body.push(if let Some(error) = &self.accelerator_probe_error {
            widgets::prose(format!(
                "Accelerators were not checked because ONNX Runtime could not be loaded ({error}). \
                 Reinstall OwlWhisp so its runtime folder is restored; downloading a model alone \
                 cannot fix this."
            ))
        } else {
            widgets::prose(
                "Available requires a working provider, compatible hardware, and model files. \
                 Need additional action always has a download or driver-install button. \
                 Missing providers belong to the app build, not the graphics driver; Info gives the probe details. GPU acceleration \
                 currently runs the Parakeet TDT 0.6B v3 model; other models use the CPU-only \
                 sherpa engine. The optional GPU encoder downloads about 2.5 GB and checks \
                 the selected accelerator with a real model run before saving it.",
            )
        });

        widgets::card(body).into()
    }

    fn microphone_card(&self) -> Element<'_, Message> {
        let selected = self
            .devices
            .iter()
            .find(|d| d.0 == self.settings.audio.input_device)
            .cloned();
        widgets::card(
            column![
                widgets::heading("Microphone"),
                pick_list(self.devices.clone(), selected, |d: DeviceChoice| {
                    Message::DeviceSelected(d.0)
                })
                .text_size(14)
                .width(Length::Fill),
                widgets::sub(
                    "System default follows whatever Windows is using, including a headset that \
                     appears later.",
                ),
                widgets::sub(
                    "A long dictation stops and transcribes after five minutes; earlier speech is preserved."
                ),
                self.mic_check(),
            ]
            .spacing(8),
        )
        .into()
    }

    /// The level meter and the switch that gives it something to show.
    ///
    /// Two facts shape this, both carried over from the web version. The meter is only alive while
    /// something is capturing, so without the switch "is my microphone working?" is unanswerable
    /// on a screen where nothing is being dictated -- which is exactly when the question gets
    /// asked. And a bar stuck at zero while the stream is open is a real finding, not a gap to
    /// paper over with an idle animation, so it is reported as one.
    fn mic_check(&self) -> Element<'_, Message> {
        let level = self.mic.level.clamp(0.0, 1.0);

        let mut body = column![
            row![
                widgets::field_label("Microphone level"),
                if self.mic.open {
                    widgets::badge_yes("listening")
                } else {
                    Space::new().into()
                },
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            meter(level),
            row![
                checkbox(self.mic.open)
                    .label("Test microphone")
                    .on_toggle_maybe(
                        (!self.mic.dictating).then_some(Message::MicTestToggled as fn(bool) -> _)
                    )
                    .size(16)
                    .text_size(14),
                widgets::sub(
                    "Opens the microphone only to move this bar. Nothing is transcribed, nothing \
                     is written to disk, nothing leaves the machine.",
                ),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(6);

        let saved = self.saved.audio.input_device.trim();
        body = body.push(widgets::sub(if saved.is_empty() {
            "This test opens the system default input.".to_string()
        } else {
            format!("This test opens the saved input, matching \u{201c}{saved}\u{201d}.")
        }));

        if let Some(e) = &self.mic.error {
            body = body.push(iced::widget::text(e.clone()).size(13).color(theme::BAD));
        }

        body = body.push(widgets::sub(match (self.mic.dictating, self.mic.open) {
            (true, _) => "Dictation has the microphone; the bar is following that.".to_string(),
            (false, true) if self.mic.peak == 0.0 => {
                "No signal yet: the bar has not moved since the microphone opened. Say something. \
                 If it stays here, this input is reaching the app as silence."
                    .to_string()
            }
            (false, true) => format!(
                "Loudest so far: {}% of the bar. Ordinary speech should reach the middle.",
                (self.mic.peak * 100.0).round() as i32
            ),
            (false, false) => "The bar only moves while something is capturing - during \
                               dictation, or while this switch is on."
                .to_string(),
        }));

        body.into()
    }

    fn sound_card(&self) -> Element<'_, Message> {
        let selected = self
            .themes
            .iter()
            .find(|t| t.value == self.settings.sound_theme)
            .cloned();
        let enabled = self.settings.sounds_enabled;

        let mut body = column![
            widgets::heading("Sound cues"),
            checkbox(enabled)
                .label("Play a sound when dictation starts and stops")
                .on_toggle(Message::SoundsToggled),
        ]
        .spacing(8);

        if enabled {
            body = body.push(
                row![
                    pick_list(self.themes.clone(), selected, |t: ThemeChoice| {
                        Message::ThemeSelected(t.value)
                    })
                    .text_size(14),
                    button(widgets::button_label("Preview start"))
                        .on_press(Message::PreviewSound(Cue::Start))
                        .style(theme::action(false)),
                    // Both, because the two are the halves of one decision. A theme is chosen by
                    // how it sounds when dictation *ends* at least as much as by how it starts,
                    // and hearing only the first half means choosing half blind.
                    button(widgets::button_label("Preview stop"))
                        .on_press(Message::PreviewSound(Cue::Stop))
                        .style(theme::action(false)),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
            body = body.push(
                row![
                    widgets::sub("Volume"),
                    slider(0.0..=1.0, self.settings.sound_volume, Message::VolumeChanged)
                        .step(0.05_f32)
                        .width(220),
                    widgets::sub(format!("{:.0}%", self.settings.sound_volume * 100.0)),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
            if let Some(t) = lw_core::sound::ALL_SOUND_THEMES
                .iter()
                .find(|t| **t == self.settings.sound_theme)
            {
                body = body.push(widgets::sub(t.description()));
            }
        }

        widgets::card(body).into()
    }

    fn overlay_card(&self) -> Element<'_, Message> {
        widgets::card(
            column![
                widgets::heading("Overlay"),
                checkbox(self.settings.overlay_enabled)
                    .label("Show a floating indicator while dictating")
                    .on_toggle(Message::OverlayToggled),
                widgets::sub(
                    "A small pill above other windows. It never takes focus and clicks pass \
                     through it.",
                ),
            ]
            .spacing(8),
        )
        .into()
    }

    fn autostart_card(&self) -> Element<'_, Message> {
        widgets::card(
            column![
                widgets::heading("Startup"),
                checkbox(self.settings.autostart)
                    .label("Launch OwlWhisp when I log in")
                    .on_toggle(Message::AutostartToggled),
                checkbox(self.settings.start_minimized)
                    .label("Start minimized to tray")
                    .on_toggle(Message::StartMinimizedToggled),
                widgets::sub(
                    "Start without opening the main window, including at login. Open it from the \
                     tray menu. Takes effect after saving and restarting. If no tray is available, \
                     the window opens normally.",
                ),
            ]
            .spacing(8),
        )
        .into()
    }

    fn save_row(&self) -> Element<'_, Message> {
        let mut r = row![
            button(widgets::button_label("Save"))
                .padding(Padding::from([8, 18]))
                .on_press_maybe(self.dirty().then_some(Message::Save))
                .style(theme::action(false)),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        // Beside the button rather than under it, which is where it was asked to be.
        if let Some(e) = &self.error {
            r = r.push(iced::widget::text(e.clone()).size(13).color(theme::BAD));
        } else if let Some(n) = &self.notice {
            r = r.push(iced::widget::text(n.clone()).size(13).color(theme::GOOD));
        } else if self.dirty() {
            r = r.push(widgets::sub("Unsaved changes."));
        }
        container(r).into()
    }
}

/// Every trigger key the editor offers, grouped the way the web version grouped them: no key at
/// all, then the four in everyday use, then letters, digits and function keys.
///
/// `pick_list` has no group headings, so the grouping survives as order alone. Everything here is
/// accepted by `lw-core`, which is what makes the list safe to offer -- there is a test below that
/// keeps the two in step.
fn provider_guide(id: &str) -> Option<(&'static str, &'static str)> {
    Some(match id {
        "cpu" => (
            "Built into OwlWhisp; no driver download is needed.",
            "https://onnxruntime.ai/docs/execution-providers/",
        ),
        "qnn_npu" => (
            "Qualcomm NPU needs its OEM Windows driver, the QNN provider bundled with OwlWhisp, and a compatible model artifact.",
            "https://onnxruntime.ai/docs/execution-providers/QNN-ExecutionProvider.html",
        ),
        "web_gpu" => (
            "Uses the GPU's system graphics driver and OwlWhisp's WebGPU provider. Parakeet needs the optional full-precision GPU encoder; Add GPU model downloads and verifies it.",
            "https://onnxruntime.ai/docs/execution-providers/WebGPU-ExecutionProvider.html",
        ),
        "cuda" => (
            "Uses the NVIDIA display driver, CUDA/cuDNN runtime packages selected and installed by OwlWhisp, and the optional full-precision Parakeet encoder. Add GPU model downloads and verifies the encoder, then checks CUDA on this machine.",
            "https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html",
        ),
        "tensor_rt" => (
            "Download runtime selects TensorRT libraries for the detected NVIDIA GPU; CUDA and TensorRT need a CUDA 13-compatible driver. Parakeet uses FP16 and a profile covering dictation windows up to 20 seconds. First preparation can take several minutes and the engine cache uses about 1.3 GB of disk. Benchmark it against CUDA on your machine.",
            "https://onnxruntime.ai/docs/execution-providers/TensorRT-ExecutionProvider.html",
        ),
        "direct_ml" => (
            "OwlWhisp's Windows x64 build includes a separate Windows ML runtime. Download runtime repairs missing DirectML components; the app checks the real device before enabling it.",
            "https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html",
        ),
        "open_vino" => (
            "OpenVINO NPU needs an Intel NPU, a compatible provider, and a model that runs on it.",
            "https://onnxruntime.ai/docs/execution-providers/OpenVINO-ExecutionProvider.html",
        ),
        "core_ml" => (
            "CoreML requires macOS and an OwlWhisp build containing its provider.",
            "https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html",
        ),
        "vitis_ai" => (
            "Ryzen AI needs a supported AMD NPU, its OEM driver, the Vitis AI provider, and a compatible model artifact.",
            "https://onnxruntime.ai/docs/execution-providers/Vitis-AI-ExecutionProvider.html",
        ),
        _ => return None,
    })
}

fn trigger_choices() -> Vec<TriggerChoice> {
    // First, because it is the one entry that changes what the rest of the card means: picked, the
    // modifier boxes *are* the binding, and there have to be two of them.
    let none = std::iter::once(String::new());
    let common = ["space", "tab", "enter", "esc"].into_iter().map(String::from);
    let letters = (b'a'..=b'z').map(|c| (c as char).to_string());
    let digits = (0..10).map(|d| d.to_string());
    let fkeys = (1..=20).map(|n| format!("f{n}"));
    none.chain(common)
        .chain(letters)
        .chain(digits)
        .chain(fkeys)
        .map(|value| TriggerChoice {
            label: if value.is_empty() {
                "No key - two modifiers instead".to_string()
            } else {
                lw_app::hotkey::trigger_label(&value)
            },
            value,
        })
        .collect()
}

/// The trigger name for a captured keystroke, or `None` for a key no binding can use.
///
/// `None` is also what a bare modifier gives, and that is the useful case: the user is still
/// holding keys down on the way to the real one, and capture waits rather than binding half a
/// combination.
fn trigger_from_key(key: &iced::keyboard::Key) -> Option<String> {
    use iced::keyboard::Key;
    use iced::keyboard::key::Named;

    const FKEYS: [(Named, &str); 20] = [
        (Named::F1, "f1"),
        (Named::F2, "f2"),
        (Named::F3, "f3"),
        (Named::F4, "f4"),
        (Named::F5, "f5"),
        (Named::F6, "f6"),
        (Named::F7, "f7"),
        (Named::F8, "f8"),
        (Named::F9, "f9"),
        (Named::F10, "f10"),
        (Named::F11, "f11"),
        (Named::F12, "f12"),
        (Named::F13, "f13"),
        (Named::F14, "f14"),
        (Named::F15, "f15"),
        (Named::F16, "f16"),
        (Named::F17, "f17"),
        (Named::F18, "f18"),
        (Named::F19, "f19"),
        (Named::F20, "f20"),
    ];

    match key {
        Key::Named(Named::Space) => Some("space".into()),
        Key::Named(Named::Tab) => Some("tab".into()),
        Key::Named(Named::Enter) => Some("enter".into()),
        Key::Named(Named::Escape) => Some("esc".into()),
        Key::Named(n) => FKEYS
            .iter()
            .find(|(named, _)| named == n)
            .map(|(_, name)| (*name).to_string()),
        Key::Character(c) => {
            let lower = c.to_lowercase();
            (lower.len() == 1 && lower.chars().all(|ch| ch.is_ascii_alphanumeric())).then_some(lower)
        }
        _ => None,
    }
}

/// A horizontal bar filled to `level`.
///
/// Two flex portions rather than a fixed width, because the card's width is not known here and a
/// meter that did not match its container would be a meter reporting the wrong number. The row
/// needs `Fill` of its own: a row defaults to shrinking to its content, and children asking for a
/// portion of nothing collapse to a line a pixel wide.
fn meter<'a, M: 'a>(level: f32) -> Element<'a, M> {
    const STEPS: u16 = 1000;
    let filled = (level.clamp(0.0, 1.0) * f32::from(STEPS)) as u16;

    let mut bar = row![].width(Length::Fill).height(8);
    if filled > 0 {
        bar = bar.push(
            container(Space::new().width(Length::Fill).height(8))
                .width(Length::FillPortion(filled))
                .style(|_t| container::Style {
                    background: Some(theme::GOOD.into()),
                    border: iced::Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        );
    }
    if filled < STEPS {
        bar = bar.push(Space::new().width(Length::FillPortion(STEPS - filled)).height(8));
    }

    container(bar)
        .width(Length::Fill)
        .height(8)
        .style(|_t| container::Style {
            background: Some(theme::BG_RAISED.into()),
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::Key;
    use iced::keyboard::key::Named;

    /// The whole capture path as the panel really runs it: arm the grab, press keys, poll.
    ///
    /// Between the keyboard hook and the binding in the panel there is a channel, a poll, a name
    /// lookup and a validation, and every one of them has been wrong at least once. The platform
    /// crate's own tests stop at the channel; this one goes the rest of the way.
    ///
    /// `cargo test -p lw-gui capture_reaches_the_panel -- --ignored --nocapture`
    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    #[cfg(windows)]
    fn a_captured_combination_reaches_the_binding_in_the_panel() {
        use windows_sys_keys::press;

        let mut state = State::new();
        state.update(Message::CaptureToggled);
        assert!(state.capturing, "the button armed the capture");
        assert!(
            state.capture.is_some(),
            "this machine has a keyboard grab and the panel should have taken it",
        );

        // Meta+D: the combination a window never gets to see, which is why the grab exists.
        press(&[0x5B, 0x44]);

        // The subscription polls every 25 ms; here the poll is driven by hand.
        for _ in 0..100 {
            state.update(Message::CapturePoll);
            if !state.capturing {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(
            !state.capturing,
            "the capture should have closed itself: {:?}",
            state.error
        );
        assert_eq!(state.settings.hotkey.trigger, "d");
        assert_eq!(state.settings.hotkey.modifiers, vec!["meta".to_string()]);
        assert_eq!(state.error, None);
    }

    /// Press every key in order and let go in reverse, physically enough for the hook to see.
    #[cfg(windows)]
    mod windows_sys_keys {
        /// Uses the raw Win32 call rather than a helper crate; `lw-gui` has no windows dependency
        /// of its own and this is the only place that wants one.
        pub fn press(vks: &[u16]) {
            unsafe extern "system" {
                fn keybd_event(bVk: u8, bScan: u8, dwFlags: u32, dwExtraInfo: usize);
            }
            const KEYEVENTF_KEYUP: u32 = 0x0002;
            for vk in vks {
                unsafe { keybd_event(*vk as u8, 0, 0, 0) };
            }
            for vk in vks.iter().rev() {
                unsafe { keybd_event(*vk as u8, 0, KEYEVENTF_KEYUP, 0) };
            }
        }
    }

    #[test]
    fn every_offered_trigger_is_one_the_core_accepts() {
        // The dropdown is a promise: pick any of these and Save will work. A key the core rejects
        // would be offered and then refused, with no way for the user to tell which.
        //
        // Two modifiers, because one of the entries is "no key" and that is the form it needs.
        for choice in trigger_choices() {
            let cfg = lw_core::settings::HotkeyConfig {
                modifiers: vec!["ctrl".into(), "meta".into()],
                trigger: choice.value.clone(),
                mode: HotkeyMode::Toggle,
            };
            assert!(
                cfg.validate().is_ok(),
                "{:?} was offered but is invalid",
                choice.value
            );
        }
    }

    #[test]
    fn the_editor_offers_the_modifiers_only_binding_and_lists_it_first() {
        let choices = trigger_choices();
        assert_eq!(choices[0].value, "", "no key belongs at the top of the list");
        assert_eq!(
            choices.iter().filter(|c| c.value.is_empty()).count(),
            1,
            "exactly one way to say it",
        );
    }

    #[test]
    fn a_binding_saved_as_none_selects_the_no_key_entry() {
        // The settings file writes "none"; the list carries "". Without the normalisation in
        // `trigger_of` the dropdown would show its placeholder for a binding that is perfectly
        // well set, and adding a second "(saved)" entry beside the one that already means it.
        let cfg = lw_core::settings::HotkeyConfig {
            modifiers: vec!["ctrl".into(), "meta".into()],
            trigger: "none".into(),
            mode: HotkeyMode::PushToTalk,
        };
        let saved = lw_app::hotkey::trigger_of(&cfg);
        assert!(trigger_choices().iter().any(|c| c.value == saved));
    }

    #[test]
    fn capture_waits_for_a_real_key_rather_than_binding_a_bare_modifier() {
        assert_eq!(trigger_from_key(&Key::Named(Named::Control)), None);
        assert_eq!(trigger_from_key(&Key::Named(Named::Shift)), None);
    }

    #[test]
    fn capture_maps_the_keys_the_editor_can_also_be_set_to_by_hand() {
        assert_eq!(
            trigger_from_key(&Key::Named(Named::Space)).as_deref(),
            Some("space")
        );
        assert_eq!(trigger_from_key(&Key::Named(Named::F13)).as_deref(), Some("f13"));
        assert_eq!(
            trigger_from_key(&Key::Character("D".into())).as_deref(),
            Some("d")
        );
        assert_eq!(
            trigger_from_key(&Key::Character("7".into())).as_deref(),
            Some("7")
        );
        // Punctuation is not in the list the core accepts, so capture must not produce it.
        assert_eq!(trigger_from_key(&Key::Character(";".into())), None);
    }
}
