//! The Dictate tab.
//!
//! A port of `app/frontend/src/panels/Dictate.tsx`: the status pill with the hotkey hint, what the
//! next dictation will actually run on, a scratchpad to try it in, and the last transcript.
//!
//! The pill is the one thing on screen that must never lie. It shows the worker's own state, not
//! an optimistic guess made when a key was pressed, and the backend line says "nothing loaded yet"
//! before the first utterance rather than naming the accelerator it hopes to use.

use iced::widget::{button, column, container, row, scrollable, text_editor, Space};
use iced::{Color, Element, Length, Padding};
use lw_app::dictation::{Command, Event, Handle, RecordingState};

use crate::{theme, widgets};

#[derive(Debug, Clone)]
pub enum Message {
    Toggle,
    Poll,
    Scratchpad(text_editor::Action),
    CopyLast,
    ClearScratchpad,
}

pub struct State {
    worker: Handle,
    state: RecordingState,
    level: f32,
    backend: Option<Box<lw_app::bench::ActiveBackend>>,
    last: Option<(String, bool, String)>,
    error: Option<String>,
    scratchpad: text_editor::Content,
    hotkey: String,
    mode_is_hands_free: bool,
    /// Rising while listening, to make the pill breathe rather than sit still.
    phase: f32,
}

impl State {
    pub fn new() -> Self {
        let settings_path = lw_app::paths::settings_path();
        let settings = lw_core::settings::Settings::load(&settings_path).unwrap_or_default();
        let worker = lw_app::dictation::spawn(settings_path);
        worker.send(Command::Describe);
        Self {
            state: RecordingState::Idle,
            level: 0.0,
            backend: None,
            last: None,
            error: None,
            scratchpad: text_editor::Content::new(),
            hotkey: settings.hotkey.to_accelerator().unwrap_or_default(),
            mode_is_hands_free: settings.hotkey.mode == lw_core::settings::HotkeyMode::HandsFree,
            phase: 0.0,
            worker,
        }
    }

    /// Drain the worker's channel on a timer.
    ///
    /// A poll rather than a bridge from the worker's crossbeam channel into a futures stream: the
    /// events are a handful per utterance plus a meter tick every 50 ms, and a 16 ms poll while
    /// the window is open is both simpler and easier to reason about than two runtimes sharing a
    /// channel. It stops when nothing is happening, so an idle window still costs nothing.
    pub fn subscription(&self) -> iced::Subscription<Message> {
        let busy = self.state != RecordingState::Idle;
        let period = if busy { 16 } else { 250 };
        iced::time::every(std::time::Duration::from_millis(period)).map(|_| Message::Poll)
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Toggle => {
                match self.state {
                    RecordingState::Listening => self.worker.send(Command::Stop),
                    RecordingState::Idle => self.worker.send(Command::Start {
                        hands_free: self.mode_is_hands_free,
                    }),
                    // Mid-transcription: pressing again must not start a second capture.
                    _ => {}
                }
            }
            Message::Poll => {
                self.phase = (self.phase + 0.05) % 1.0;
                while let Ok(ev) = self.worker.events.try_recv() {
                    match ev {
                        Event::State(s) => self.state = s,
                        Event::Level(l) => self.level = l,
                        Event::Backend(b) => self.backend = Some(b),
                        Event::Error(e) => self.error = Some(e),
                        Event::MicTest(_) => {}
                        Event::Transcript {
                            text,
                            injected,
                            provider,
                        } => {
                            self.error = None;
                            self.last = Some((text, injected, provider));
                            // Ask again: the first utterance is what loads the engine, so this is
                            // the moment the backend line stops being "nothing loaded yet".
                            self.worker.send(Command::Describe);
                        }
                    }
                }
            }
            Message::Scratchpad(action) => self.scratchpad.perform(action),
            Message::CopyLast => {
                if let Some((text, _, _)) = &self.last {
                    if let Ok(mut clip) = lw_platform::platform().clipboard() {
                        use lw_platform::Clipboard;
                        let _ = clip.set_text(text);
                    }
                }
            }
            Message::ClearScratchpad => self.scratchpad = text_editor::Content::new(),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        scrollable(
            column![
                self.pill(),
                self.backend_line(),
                self.scratchpad_card(),
                self.last_card(),
                widgets::prose(
                    "If the bar above does not change when you press the hotkey, or nothing is \
                     transcribed, check the microphone: Settings has the input picker.",
                ),
                Space::new(0, 8),
            ]
            .spacing(12)
            .padding(Padding::from([0, 8])),
        )
        .height(Length::Fill)
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
            RecordingState::Listening => 0.65 + 0.35 * (self.phase * std::f32::consts::TAU).sin().abs(),
            _ => 1.0,
        };

        let level_bar = if self.state == RecordingState::Listening {
            let w = (self.level.clamp(0.0, 1.0) * 160.0).max(2.0);
            container(Space::new(w, 4))
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
            Element::from(Space::new(0, 4))
        };

        container(
            row![
                iced::widget::text(self.state.label()).size(17).color(fg),
                level_bar,
                Space::new(Length::Fill, 0),
                widgets::sub(if self.hotkey.is_empty() {
                    "No hotkey set".to_string()
                } else {
                    format!("Press {} to start, press again to stop", self.hotkey)
                }),
            ]
            .spacing(12)
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
                row![
                    iced::widget::text("TRY DICTATING HERE")
                        .size(11)
                        .color(theme::TEXT_DIM),
                    Space::new(Length::Fill, 0),
                    button(widgets::body(match self.state {
                        RecordingState::Listening => "Stop",
                        _ => "Start",
                    }))
                    .padding(Padding::from([6, 14]))
                    .on_press(Message::Toggle),
                    button(widgets::body("Clear"))
                        .padding(Padding::from([6, 14]))
                        .on_press(Message::ClearScratchpad),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
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
            Some((text, injected, provider)) => column![
                row![
                    if *injected {
                        widgets::badge_yes("typed into the focused window")
                    } else {
                        widgets::badge("copied to the clipboard", theme::ESTIMATE)
                    },
                    widgets::sub(provider.clone()),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
                widgets::body(text.clone()),
                button(widgets::body("Copy"))
                    .padding(Padding::from([6, 14]))
                    .on_press(Message::CopyLast),
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
