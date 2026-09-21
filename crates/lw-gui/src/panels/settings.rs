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

use iced::widget::{button, checkbox, column, container, pick_list, radio, row, slider, Space};
use iced::{Element, Length, Padding};
use lw_core::engine::BackendPreference;
use lw_core::settings::{HotkeyMode, Settings};
use lw_core::sound::{Cue, SoundTheme};

use crate::{theme, widgets};

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

pickable!(ModelChoice, String);
pickable!(BackendChoice, BackendPreference);
pickable!(ThemeChoice, SoundTheme);
pickable!(ModeChoice, HotkeyMode);
pickable!(TriggerChoice, String);

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
    ModelSelected(String),
    BackendSelected(BackendPreference),
    DeviceSelected(String),
    ModeSelected(HotkeyMode),
    TriggerSelected(String),
    ModifierToggled(&'static str, bool),
    /// Start or abandon "press the combination you want".
    CaptureToggled,
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
    PreviewSound,
    AutostartToggled(bool),
    Save,
    Reload,
}

pub struct State {
    settings: Settings,
    /// What is on disk, to tell whether anything is unsaved.
    saved: Settings,
    devices: Vec<DeviceChoice>,
    models: Vec<ModelChoice>,
    backends: Vec<BackendChoice>,
    themes: Vec<ThemeChoice>,
    modes: Vec<ModeChoice>,
    /// Every trigger key the editor offers, in the order it offers them.
    triggers: Vec<TriggerChoice>,
    /// Whether the next keystroke should be read as a new binding rather than typed.
    capturing: bool,
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
    notice: Option<String>,
    error: Option<String>,
}

impl State {
    pub fn new() -> Self {
        let path = lw_app::paths::settings_path();
        let settings = Settings::load(&path).unwrap_or_default();
        let diag = lw_app::diagnostics::collect(env!("CARGO_PKG_VERSION"));
        let usable = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.usable))
            .collect();

        let models = lw_app::catalog::build(&lw_app::paths::models_root())
            .map(|v| {
                v.entries
                    .iter()
                    .map(|e| ModelChoice {
                        value: e.id.clone(),
                        label: e.name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            saved: settings.clone(),
            settings,
            devices: std::iter::once(DeviceChoice(String::new()))
                .chain(lw_platform::audio::list_input_devices().into_iter().map(DeviceChoice))
                .collect(),
            models,
            backends: BackendPreference::all()
                .into_iter()
                .map(|p| BackendChoice {
                    value: p,
                    label: p.label().to_string(),
                })
                .collect(),
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
                (HotkeyMode::HandsFree, "Hands free - tap to start, silence ends it"),
            ]
            .into_iter()
            .map(|(value, label)| ModeChoice {
                value,
                label: label.to_string(),
            })
            .collect(),
            triggers: trigger_choices(),
            capturing: false,
            width: 1000.0,
            hotkey_status: lw_app::hotkey::Status::default(),
            mic: Mic::default(),
            usable,
            notice: None,
            error: None,
        }
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

    /// Listen for the next keystroke, but only while the user asked to be listened to.
    ///
    /// Not a permanent listener: this window has ordinary text fields in it, and a panel that read
    /// every keypress as a hotkey would rebind the shortcut while someone typed a model name.
    pub fn subscription(&self) -> iced::Subscription<Message> {
        if !self.capturing {
            return iced::Subscription::none();
        }
        // `on_key_press` is gone in iced 0.14; `listen` reports every keyboard event and the
        // press is picked out here. Same events, one more line.
        iced::keyboard::listen().filter_map(|event| {
            let iced::keyboard::Event::KeyPressed {
                key, modifiers, ..
            } = event
            else {
                return None;
            };
            // Esc on its own cancels, exactly as it did in the web editor. Esc *with* a modifier is
            // a perfectly good trigger key and is taken as one.
            if key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
                && modifiers.is_empty()
            {
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
            Message::ModelSelected(id) => self.settings.model_id = id,
            Message::BackendSelected(b) => self.settings.backend = b,
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
            Message::CaptureToggled => self.capturing = !self.capturing,
            Message::Resized(w) => self.width = w,
            Message::VadThreshold(v) => self.settings.vad.threshold = v,
            // Nothing is set here. The shell forwards this to the worker, and the answer comes
            // back through `set_mic` -- the switch shows what happened, not what was asked.
            Message::MicTestToggled(_) => {}
            Message::Captured { modifiers, trigger } => {
                self.settings.hotkey.modifiers = modifiers;
                self.settings.hotkey.trigger = trigger;
                self.capturing = false;
            }
            Message::OverlayToggled(v) => self.settings.overlay_enabled = v,
            Message::SoundsToggled(v) => self.settings.sounds_enabled = v,
            Message::ThemeSelected(t) => self.settings.sound_theme = t,
            Message::VolumeChanged(v) => self.settings.sound_volume = v,
            Message::PreviewSound => {
                lw_platform::play_cue(
                    self.settings.sound_theme,
                    Cue::Start,
                    self.settings.sound_volume,
                );
            }
            Message::AutostartToggled(v) => {
                // Written through the platform and then read back, so the checkbox can never claim
                // a registration the OS refused.
                match lw_platform::autostart::set(v) {
                    Ok(()) => match lw_platform::autostart::is_enabled() {
                        Ok(actual) => {
                            self.settings.autostart = actual;
                            if actual != v {
                                self.error = Some(
                                    "The operating system refused to change login start.".into(),
                                );
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
                        self.saved = self.settings.clone();
                        self.notice = Some("Saved.".into());
                        self.error = None;
                        wrote = true;
                    }
                    Err(e) => self.error = Some(format!("Could not save: {e}")),
                }
            }
            Message::Reload => *self = Self::new(),
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
            self.model_card(),
            self.sound_card(),
            self.overlay_card(),
            self.autostart_card(),
        ];
        let hardware: Vec<Element<'_, Message>> = vec![
            self.accelerator_card(),
            self.microphone_card(),
            self.vad_card(),
        ];

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
            button(widgets::body(if self.capturing {
                "Cancel capture"
            } else {
                "Capture keystroke"
            }))
            .padding(Padding::from([6, 14]))
            .on_press(Message::CaptureToggled),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        let mut card = column![widgets::heading("Hotkey"), preview].spacing(8);
        if self.capturing {
            card = card.push(widgets::sub(
                "Press the combination you want to use. Esc on its own cancels.",
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
                checkbox(lw_app::hotkey::has_modifier(h, m.id)).label(label)
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
                .placeholder("No key - pick one")
                .text_size(14)
                .width(220),
                widgets::sub(
                    "A global shortcut needs a real key. Modifiers on their own cannot be \
                     registered with the OS, so every binding pairs them with one of these.",
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
            (Some(spec), None) => row![
                widgets::mono(
                    spec.modifiers
                        .iter()
                        .map(|m| lw_app::hotkey::modifier_label(m))
                        .chain(std::iter::once(lw_app::hotkey::trigger_label(&spec.trigger)))
                        .collect::<Vec<_>>()
                        .join(" + "),
                ),
                widgets::sub("is the combination the OS currently holds."),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into(),
            (None, None) => widgets::sub("Checking...").into(),
        }
    }

    fn model_card(&self) -> Element<'_, Message> {
        let selected = self
            .models
            .iter()
            .find(|m| m.value == self.settings.model_id)
            .cloned();
        widgets::card(
            column![
                widgets::heading("Model"),
                widgets::sub("Which model transcribes. The Models tab has the numbers."),
                pick_list(self.models.clone(), selected, |m: ModelChoice| {
                    Message::ModelSelected(m.value)
                })
                .text_size(14)
                .width(Length::Fill),
            ]
            .spacing(8),
        )
        .into()
    }

    fn accelerator_card(&self) -> Element<'_, Message> {
        let selected = self
            .backends
            .iter()
            .find(|b| b.value == self.settings.backend)
            .cloned();

        let mut body = column![
            widgets::heading("Accelerator"),
            widgets::sub("What runs the model. Automatic picks the fastest one that works."),
            pick_list(self.backends.clone(), selected, |b: BackendChoice| {
                Message::BackendSelected(b.value)
            })
            .text_size(14)
            .width(Length::Fill),
        ]
        .spacing(8);

        // Say which of the offered choices this machine can actually honour. A picker that lets
        // someone select an NPU that silently falls back to the CPU is the exact failure this
        // project refuses to ship.
        for b in &self.backends {
            let Some(accel) = b.value.accelerator() else {
                continue;
            };
            let usable = self.usable.get(accel.id()).copied().unwrap_or(false);
            body = body.push(
                row![
                    if usable {
                        widgets::badge_yes("available")
                    } else {
                        widgets::badge_no("unavailable")
                    },
                    widgets::sub(b.label.clone()),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }
        body = body.push(widgets::prose(
            "Availability comes from a real probe of this machine, not from the model's claims. \
             The Diagnostics tab shows the whole picture, including providers that registered but \
             enumerated no device.",
        ));

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
                checkbox(self.mic.open).label("Test microphone")
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
            checkbox(enabled).label("Play a sound when dictation starts and stops")
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
                    button(widgets::body("Preview")).on_press(Message::PreviewSound),
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
                checkbox(self.settings.overlay_enabled).label("Show a floating indicator while dictating")
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
                widgets::heading("Start with the computer"),
                checkbox(self.settings.autostart).label("Launch LocalWisper when I log in")
                    .on_toggle(Message::AutostartToggled),
                widgets::sub(
                    "Read back from the operating system after being set, so this checkbox cannot \
                     claim a registration that a managed machine refused.",
                ),
            ]
            .spacing(8),
        )
        .into()
    }

    fn save_row(&self) -> Element<'_, Message> {
        let mut r = row![
            button(widgets::body("Save"))
                .padding(Padding::from([8, 18]))
                .on_press_maybe(self.dirty().then_some(Message::Save)),
            button(widgets::body("Reload"))
                .padding(Padding::from([8, 18]))
                .on_press(Message::Reload),
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

/// Every trigger key the editor offers, grouped the way the web version grouped them: the four in
/// everyday use first, then letters, digits and function keys.
///
/// `pick_list` has no group headings, so the grouping survives as order alone. Everything here is
/// accepted by `lw-core`'s `key_code_name`, which is what makes the list safe to offer -- there is
/// a test below that keeps the two in step.
fn trigger_choices() -> Vec<TriggerChoice> {
    let common = ["space", "tab", "enter", "esc"].into_iter().map(String::from);
    let letters = (b'a'..=b'z').map(|c| (c as char).to_string());
    let digits = (0..10).map(|d| d.to_string());
    let fkeys = (1..=20).map(|n| format!("f{n}"));
    common
        .chain(letters)
        .chain(digits)
        .chain(fkeys)
        .map(|value| TriggerChoice {
            label: lw_app::hotkey::trigger_label(&value),
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
            (lower.len() == 1 && lower.chars().all(|ch| ch.is_ascii_alphanumeric()))
                .then_some(lower)
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

    #[test]
    fn every_offered_trigger_is_one_the_core_accepts() {
        // The dropdown is a promise: pick any of these and Save will work. A key the core rejects
        // would be offered and then refused, with no way for the user to tell which.
        for choice in trigger_choices() {
            let cfg = lw_core::settings::HotkeyConfig {
                modifiers: vec!["ctrl".into()],
                trigger: choice.value.clone(),
                mode: HotkeyMode::Toggle,
            };
            assert!(
                cfg.validate().is_ok(),
                "{} was offered but is invalid",
                choice.value
            );
        }
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
        assert_eq!(
            trigger_from_key(&Key::Named(Named::F13)).as_deref(),
            Some("f13")
        );
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
