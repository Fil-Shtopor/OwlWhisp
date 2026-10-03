//! The Dictate tab.
//!
//! A port of `app/frontend/src/panels/Dictate.tsx`: the status pill with the hotkey hint, what the
//! next dictation will actually run on, a scratchpad to try it in, and the last transcript.
//!
//! The pill is the one thing on screen that must never lie. It shows the worker's own state, not
//! an optimistic guess made when a key was pressed, and the backend line says "nothing loaded yet"
//! before the first utterance rather than naming the accelerator it hopes to use. The same rule
//! covers the hotkey hint: it is built from the binding that is actually registered, in the mode
//! that is actually in force, and it says so when the registration failed -- an instruction to
//! press keys that do nothing is the worst line this panel could print.

use iced::widget::{button, column, container, row, text_editor, Space};
use iced::{Color, Element, Length, Padding};
use lw_app::dictation::{Command, Delivery, Event, Handle, RecordingState};
use lw_core::settings::HotkeyConfig;

use crate::{theme, widgets};

/// How long one breath takes. The web version animated this in CSS over 1.2 s, and that is the
/// speed the pill is meant to have -- slow enough to read out of the corner of an eye.
const PULSE: std::time::Duration = std::time::Duration::from_millis(1200);

#[derive(Debug, Clone)]
pub enum Message {
    Poll,
    Scratchpad(text_editor::Action),
    CopyLast,
}

pub struct State {
    worker: Handle,
    state: RecordingState,
    level: f32,
    target_level: f32,
    backend: Option<Box<lw_app::bench::ActiveBackend>>,
    last: Option<(String, Delivery, String)>,
    error: Option<String>,
    scratchpad: text_editor::Content,
    /// The binding as configured, which is what the hint is written from.
    hotkey: HotkeyConfig,
    /// The pump that turns key edges into worker commands, whatever tab is showing and whether or
    /// not this window has focus. Owned here because the worker is; dropping it unregisters.
    pump: lw_app::hotkey::Pump,
    /// Whether the OS actually accepted the binding, re-read on every poll.
    registered: bool,
    /// Mirrors `Settings.overlay_enabled`; the floating indicator is the user's to switch off.
    overlay_enabled: bool,
    /// Whether the microphone-test stream is open, *as the worker reported it* -- never as it was
    /// asked for. A machine can refuse access, and a saved device that is no longer plugged in
    /// makes the call fail outright.
    mic_test: bool,
    /// Why the test stream would not open.
    mic_error: Option<String>,
    /// When the panel started, so the breath can be a function of the clock.
    ///
    /// It used to be a counter bumped once per poll, which made its speed a side effect of the
    /// poll rate: 16 ms a tick and 0.05 a step is a full cycle every 320 ms, and the main pill's
    /// `abs(sin)` halved that again to 160 ms. That is a strobe, not a breath.
    started: std::time::Instant,
}

impl State {
    pub fn new() -> Self {
        let settings_path = lw_app::paths::settings_path();
        let settings = lw_core::settings::Settings::load(&settings_path).unwrap_or_default();
        let worker = lw_app::dictation::spawn(settings_path);
        worker.send(Command::Describe);
        let pump = lw_app::hotkey::Pump::spawn(&settings.hotkey, worker.remote());
        Self {
            state: RecordingState::Idle,
            level: 0.0,
            target_level: 0.0,
            backend: None,
            last: None,
            error: None,
            scratchpad: text_editor::Content::new(),
            hotkey: settings.hotkey,
            registered: pump.status().is_live(),
            overlay_enabled: settings.overlay_enabled,
            mic_test: false,
            mic_error: None,
            pump,
            started: std::time::Instant::now(),
            worker,
        }
    }

    /// What the worker is doing, for the overlay to mirror.
    pub fn state(&self) -> RecordingState {
        self.state
    }

    /// The breathing phase, 0..1 over [`PULSE`], shared with the overlay so the two pills pulse
    /// together rather than drifting apart on two timers.
    pub fn phase(&self) -> f32 {
        (self.started.elapsed().as_secs_f32() / PULSE.as_secs_f32()) % 1.0
    }

    /// Whether the user wants a floating indicator at all.
    ///
    /// Not "is there something to indicate": the window exists for as long as the setting is on,
    /// and it is the pill inside it that appears and disappears. See `App::sync_overlay`.
    pub fn overlay_enabled(&self) -> bool {
        self.overlay_enabled
    }

    /// The live input level, 0..1, which is only non-zero while something is capturing.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Open or close the microphone-test stream.
    ///
    /// Nothing is assumed from the request: the flag this panel reports comes back from the
    /// worker, so a machine that refuses the microphone leaves the switch off rather than on and
    /// lying.
    pub fn set_mic_test(&mut self, on: bool) {
        if on && self.state != RecordingState::Idle {
            // Dictation has it. Asking for a second capture at that moment is pointless, and on
            // some devices worse than pointless.
            return;
        }
        self.mic_error = None;
        tracing::info!(requested_open = on, "microphone test requested");
        self.worker.send(Command::MicTest(on));
    }

    /// Whether the test stream is open, according to the worker.
    pub fn mic_test(&self) -> bool {
        self.mic_test
    }

    /// Why it is not, if it was asked for and did not open.
    pub fn mic_error(&self) -> Option<&str> {
        self.mic_error.as_deref()
    }

    /// What the OS made of the binding, for the Settings tab to show.
    pub fn hotkey_status(&self) -> lw_app::hotkey::Status {
        self.pump.status()
    }

    /// Settings were saved: re-read them, tell the worker, and move the hotkey if it moved.
    ///
    /// Called by the shell rather than discovered here. A panel that polled `settings.json` would
    /// be reading a file sixty times a second to learn something the application already knows.
    pub fn settings_saved(&mut self) {
        let settings = lw_core::settings::Settings::load(&lw_app::paths::settings_path())
            .unwrap_or_default();
        self.worker.send(Command::ReloadSettings);
        self.overlay_enabled = settings.overlay_enabled;
        if settings.hotkey != self.hotkey {
            self.pump.rebind(&settings.hotkey);
            self.hotkey = settings.hotkey;
        }
        self.registered = self.pump.status().is_live();
    }

    /// Drain the worker's channel on a timer.
    ///
    /// A poll rather than a bridge from the worker's crossbeam channel into a futures stream: the
    /// events are a handful per utterance plus a meter tick every 50 ms, and a 16 ms poll while
    /// the window is open is both simpler and easier to reason about than two runtimes sharing a
    /// channel. It stops when nothing is happening, so an idle window still costs nothing.
    pub fn subscription(&self) -> iced::Subscription<Message> {
        let busy = self.state != RecordingState::Idle || self.mic_test;
        let period = if busy { 16 } else { 250 };
        iced::time::every(std::time::Duration::from_millis(period)).map(|_| Message::Poll)
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Poll => {
                self.registered = self.pump.status().is_live();
                while let Ok(ev) = self.worker.events.try_recv() {
                    match ev {
                        Event::State(s) => {
                            // Dictation takes the microphone back. Close the test stream rather
                            // than leave a switch claiming one that was taken away underneath it.
                            if s != RecordingState::Idle && self.mic_test {
                                self.worker.send(Command::MicTest(false));
                            }
                            self.state = s;
                        }
                        Event::Level(l) => self.target_level = l.clamp(0.0, 1.0),
                        Event::Backend(b) => self.backend = Some(b),
                        Event::Error(e) => self.error = Some(e),
                        Event::MicTest(Ok(open)) => {
                            self.mic_test = open;
                            // `false` is the normal acknowledgement when the user switches the
                            // test off. An opening failure is sent as `Err` below.
                            self.mic_error = None;
                            if !open {
                                self.level = 0.0;
                                self.target_level = 0.0;
                            }
                        }
                        Event::MicTest(Err(e)) => {
                            tracing::warn!(error = %e, "microphone test failed");
                            self.mic_test = false;
                            self.mic_error = Some(e);
                            self.level = 0.0;
                            self.target_level = 0.0;
                        }
                        Event::Transcript {
                            text,
                            delivery,
                            provider,
                        } => {
                            self.error = None;
                            // Our own window had focus, so the text comes here rather than through
                            // a synthetic paste: this panel owns the box the user was typing in.
                            if delivery == Delivery::OwnWindow {
                                self.scratchpad.perform(text_editor::Action::Edit(
                                    text_editor::Edit::Paste(std::sync::Arc::new(text.clone())),
                                ));
                            }
                            self.last = Some((text, delivery, provider));
                            // Ask again: the first utterance is what loads the engine, so this is
                            // the moment the backend line stops being "nothing loaded yet".
                            self.worker.send(Command::Describe);
                        }
                    }
                }
                if self.mic_test || self.state == RecordingState::Listening {
                    // The audio source updates every 50 ms. Move the displayed value on each
                    // 16 ms UI tick, with a quick attack and slower release.
                    let weight = if self.target_level > self.level { 0.4 } else { 0.12 };
                    self.level += (self.target_level - self.level) * weight;
                } else {
                    self.level = 0.0;
                    self.target_level = 0.0;
                }
            }
            Message::Scratchpad(action) => self.scratchpad.perform(action),
            Message::CopyLast => {
                if let (Some((text, _, _)), Ok(mut clip)) =
                    (&self.last, lw_platform::platform().clipboard())
                {
                    let _ = clip.set_text(text);
                }
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        column![
                self.pill(),
                self.backend_line(),
                self.scratchpad_card(),
                self.last_card(),
                widgets::prose(
                    "If the bar above does not change when you press the hotkey, or nothing is \
                     transcribed, check the microphone: Settings has the input picker.",
                ),
                Space::new().height(8),
            ]
        .spacing(12)
        .padding(Padding::from([0, 8]))
        .into()
    }

    /// The status pill: an oval that fills with colour while listening and breathes while it does.
    fn pill(&self) -> Element<'_, Message> {
        let (bg, fg) = match self.state {
            RecordingState::Listening => (theme::BAD, Color::WHITE),
            RecordingState::Processing => (theme::ESTIMATE, theme::BG),
            RecordingState::Error => (theme::BAD, Color::WHITE),
            RecordingState::Done => (theme::GOOD, theme::BG),
            RecordingState::Idle => (theme::BG_RAISED, theme::TEXT),
        };
        // Breathing, not blinking: a hard on/off reads as a fault, and this has to be legible from
        // the corner of the eye while the user is looking at whatever they are dictating into.
        let alpha = match self.state {
            // One cycle per `PULSE`, not two: `abs(sin)` would fold the wave and blink twice as
            // often as the overlay beside it.
            RecordingState::Listening => {
                0.65 + 0.35 * (0.5 + 0.5 * (self.phase() * std::f32::consts::TAU).cos())
            }
            _ => 1.0,
        };

        let level_bar = if self.state == RecordingState::Listening {
            let w = (self.level.clamp(0.0, 1.0) * 160.0).max(2.0);
            container(Space::new().width(w).height(4))
                .style(|_t| container::Style {
                    background: Some(Color { a: 0.85, ..Color::WHITE }.into()),
                    border: iced::Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        } else {
            Element::from(Space::new().height(4))
        };

        container(
            row![
                iced::widget::text(self.state.label()).size(17).color(fg),
                level_bar,
                Space::new().width(Length::Fill),
                self.hotkey_hint(),
            ]
            .spacing(12)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([14, 20]))
        .width(Length::Fill)
        .style(move |_t| container::Style {
            background: Some(theme::faded(bg, alpha).into()),
            border: iced::Border {
                radius: 999.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    }

    /// The one line that tells the user how to dictate.
    ///
    /// Two separate facts, and neither may be inferred from the other: what the binding *says*,
    /// which comes from settings, and whether the OS *took* it, which comes from the pump. A
    /// combination another application already owns registers as nothing at all, and the user is
    /// owed that in the same breath as the keys -- otherwise the only symptom is a hotkey that
    /// does nothing, which reads as a broken microphone.
    fn hotkey_hint(&self) -> Element<'_, Message> {
        let sentence = lw_app::hotkey::hint(&self.hotkey);
        if self.registered || lw_app::hotkey::trigger_of(&self.hotkey).is_empty() {
            return widgets::sub(sentence).into();
        }
        row![
            widgets::sub(sentence),
            iced::widget::text("- not registered with the OS")
                .size(13)
                .color(theme::BAD),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center)
        .into()
    }

    fn backend_line(&self) -> Element<'_, Message> {
        let (badge, text) = match self.backend.as_deref() {
            Some(b) if b.loaded => (
                widgets::badge_yes("running on"),
                format!(
                    "{} on {}{}",
                    b.provider.clone().unwrap_or_default(),
                    b.acceleration.clone().unwrap_or_default(),
                    b.device
                        .as_ref()
                        .map(|d| format!(" ({d})"))
                        .unwrap_or_default(),
                ),
            ),
            Some(b) if b.error.is_some() => (
                widgets::badge_no("error"),
                b.error.clone().unwrap_or_default(),
            ),
            // Before the first utterance nothing is loaded, and saying which accelerator it
            // *would* use is a prediction. It is labelled as one rather than shown as a fact.
            _ => (
                widgets::badge("predicted", theme::TEXT_DIM),
                "Nothing loaded yet - the first dictation loads the model and this becomes a fact."
                    .to_string(),
            ),
        };

        let mut c = column![row![badge, widgets::sub(text)]
            .spacing(8)
            .align_y(iced::Alignment::Center)]
        .spacing(4);
        if let Some(b) = self.backend.as_deref() {
            for note in &b.notes {
                c = c.push(widgets::sub(note.clone()));
            }
        }
        if let Some(e) = &self.error {
            c = c.push(iced::widget::text(e.clone()).size(13).color(theme::BAD));
        }
        c.into()
    }

    fn scratchpad_card(&self) -> Element<'_, Message> {
        widgets::card(
            column![
                // No Start button, deliberately. Dictation is a hotkey; a button that starts it
                // is also a button that takes the focus off the very box the text is meant to
                // land in, so pressing it and then speaking put the words nowhere. The hotkey
                // is the interface, and the line above says what it is.
                widgets::field_label("Try dictating here"),
                text_editor(&self.scratchpad)
                    .height(160)
                    .placeholder(
                        "Click here first, then use your hotkey and speak. The text arrives the \
                         same way it would in any other app."
                    )
                    .on_action(Message::Scratchpad),
                widgets::sub(
                    "A scratchpad. Nothing typed or dictated here is saved, logged or sent \
                     anywhere - it is gone when this window closes.",
                ),
            ]
            .spacing(8),
        )
        .into()
    }

    fn last_card(&self) -> Element<'_, Message> {
        let body: Element<'_, Message> = match &self.last {
            None => widgets::sub("Nothing has been dictated yet in this session.").into(),
            // An empty result is a real outcome and deserves a sentence, not an empty card with a
            // badge claiming the nothing was put somewhere.
            Some((_, Delivery::Nothing, _)) => widgets::prose(
                "That recording produced no words. The microphone was open and the model ran, so \
                 either nothing was said or nothing reached it - the level meter in Settings says \
                 which.",
            ),
            Some((text, delivery, provider)) => column![
                row![
                    match delivery {
                        Delivery::OwnWindow => widgets::badge_yes("put in the box above"),
                        Delivery::Injected =>
                            widgets::badge_yes("typed into the focused window"),
                        _ => widgets::badge("copied to the clipboard", theme::ESTIMATE),
                    },
                    widgets::sub(provider.clone()),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
                widgets::body(text.clone()),
                button(widgets::button_label("Copy"))
                    .padding(Padding::from([6, 14]))
                    .on_press(Message::CopyLast)
                    .style(theme::action(false)),
            ]
            .spacing(6)
            .into(),
        };
        widgets::card(
            column![
                iced::widget::text("LAST TRANSCRIPT")
                    .size(11)
                    .color(theme::TEXT_DIM),
                body,
            ]
            .spacing(6),
        )
        .into()
    }
}
