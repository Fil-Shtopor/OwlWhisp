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

use iced::widget::{button, checkbox, column, container, pick_list, row, scrollable, slider, Space};
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
    TriggerChanged(String),
    ModifierToggled(&'static str, bool),
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
            usable,
            notice: None,
            error: None,
        }
    }

    fn dirty(&self) -> bool {
        self.settings != self.saved
    }

    pub fn update(&mut self, message: Message) {
        self.notice = None;
        match message {
            Message::ModelSelected(id) => self.settings.model_id = id,
            Message::BackendSelected(b) => self.settings.backend = b,
            Message::DeviceSelected(d) => self.settings.audio.input_device = d,
            Message::ModeSelected(m) => self.settings.hotkey.mode = m,
            Message::TriggerChanged(t) => self.settings.hotkey.trigger = t,
            Message::ModifierToggled(name, on) => {
                // Remove every spelling of this modifier before adding the canonical one, or
                // ticking a box that was loaded as "control" would leave both in the list.
                let aliases: &[&str] = match name {
                    "ctrl" => &["ctrl", "control"],
                    "alt" => &["alt", "option"],
                    "shift" => &["shift"],
                    _ => &["win", "super", "cmd", "meta"],
                };
                let m = &mut self.settings.hotkey.modifiers;
                m.retain(|x| !aliases.iter().any(|a| x.eq_ignore_ascii_case(a)));
                if on {
                    m.push(name.to_string());
                }
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
                    }
                    Err(e) => self.error = Some(format!("Could not save: {e}")),
                }
            }
            Message::Reload => *self = Self::new(),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let mut body = column![].spacing(12).padding(Padding::from([0, 8]));

        body = body.push(self.hotkey_card());
        body = body.push(self.model_card());
        body = body.push(self.accelerator_card());
        body = body.push(self.microphone_card());
        body = body.push(self.sound_card());
        body = body.push(self.overlay_card());
        body = body.push(self.autostart_card());
        body = body.push(self.save_row());
        body = body.push(Space::new(0, 8));

        scrollable(body).height(Length::Fill).into()
    }

    fn hotkey_card(&self) -> Element<'_, Message> {
        let h = &self.settings.hotkey;
        // The spellings the core accepts, not one of them: settings.json in the wild holds "ctrl"
        // while the parser also takes "control", and checking for a single spelling left the box
        // unticked beside a hotkey that was plainly set.
        let has = |names: &[&str]| {
            h.modifiers
                .iter()
                .any(|m| names.iter().any(|n| m.eq_ignore_ascii_case(n)))
        };

        let mods = row![
            checkbox("Ctrl", has(&["ctrl", "control"]))
                .on_toggle(|v| Message::ModifierToggled("ctrl", v)),
            checkbox("Alt", has(&["alt", "option"]))
                .on_toggle(|v| Message::ModifierToggled("alt", v)),
            checkbox("Shift", has(&["shift"]))
                .on_toggle(|v| Message::ModifierToggled("shift", v)),
            checkbox("Win", has(&["win", "super", "cmd", "meta"]))
                .on_toggle(|v| Message::ModifierToggled("win", v)),
        ]
        .spacing(14);

        let selected_mode = self.modes.iter().find(|m| m.value == h.mode).cloned();

        widgets::card(
            column![
                widgets::heading("Hotkey"),
                widgets::sub("The key that starts and stops dictation, anywhere in the system."),
                mods,
                row![
                    widgets::sub("Key"),
                    iced::widget::text_input("Space", &h.trigger)
                        .on_input(Message::TriggerChanged)
                        .width(160),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
                pick_list(self.modes.clone(), selected_mode, |m: ModeChoice| {
                    Message::ModeSelected(m.value)
                })
                .text_size(14),
                widgets::prose(
                    "Hands free needs voice activity detection to decide the utterance ended, so \
                     it stops on silence rather than on a second press.",
                ),
            ]
            .spacing(8),
        )
        .into()
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
            ]
            .spacing(8),
        )
        .into()
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
            checkbox("Play a sound when dictation starts and stops", enabled)
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
                checkbox(
                    "Show a floating indicator while dictating",
                    self.settings.overlay_enabled
                )
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
                checkbox("Launch LocalWisper when I log in", self.settings.autostart)
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
