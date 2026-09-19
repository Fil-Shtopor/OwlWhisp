//! The window: a tab bar and whichever panel is selected.
//!
//! The tab order is the one the web front end settled on -- Dictate, Settings, Models, Benchmark,
//! Diagnostics -- because it is the order of how often a tab is wanted, and changing it would be a
//! change a user can see in a commit that is supposed to change nothing they can see.

use iced::widget::{button, column, container, row, text, Space};
use iced::{Element, Length, Padding};

use crate::{panels, theme};

/// Which panel is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Dictate,
    Settings,
    Models,
    Benchmark,
    Diagnostics,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Dictate,
        Tab::Settings,
        Tab::Models,
        Tab::Benchmark,
        Tab::Diagnostics,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Dictate => "Dictate",
            Tab::Settings => "Settings",
            Tab::Models => "Models",
            Tab::Benchmark => "Benchmark",
            Tab::Diagnostics => "Diagnostics",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    TabSelected(Tab),
    Resized(f32),
    Models(panels::models::Message),
    Diagnostics(panels::diagnostics::Message),
    Settings(panels::settings::Message),
    Benchmark(panels::benchmark::Message),
    Dictate(panels::dictate::Message),
}

pub struct App {
    tab: Tab,
    models: panels::models::State,
    diagnostics: panels::diagnostics::State,
    settings: panels::settings::State,
    benchmark: panels::benchmark::State,
    dictate: panels::dictate::State,
}

impl Default for App {
    fn default() -> Self {
        Self {
            tab: Tab::Dictate,
            models: panels::models::State::new(),
            diagnostics: panels::diagnostics::State::new(),
            settings: panels::settings::State::new(),
            benchmark: panels::benchmark::State::new(),
            dictate: panels::dictate::State::new(),
        }
    }
}

impl App {
    pub fn title(&self) -> String {
        "LocalWisper".to_string()
    }

    pub fn theme(&self) -> iced::Theme {
        theme::theme()
    }

    /// Resize events, so panels that lay out by breakpoint know the width.
    ///
    /// The width has to come from here rather than from iced's `responsive`, which measures its
    /// own parent and reports nothing usable inside a scrollable.
    pub fn subscription(&self) -> iced::Subscription<Message> {
        iced::Subscription::batch([
            iced::event::listen_with(|event, _status, _id| match event {
                iced::Event::Window(iced::window::Event::Resized(size)) => {
                    Some(Message::Resized(size.width))
                }
                _ => None,
            }),
            self.benchmark.subscription().map(Message::Benchmark),
            self.dictate.subscription().map(Message::Dictate),
            self.settings.subscription().map(Message::Settings),
        ])
    }

    pub fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::TabSelected(tab) => self.tab = tab,
            Message::Resized(w) => {
                // Minus the window chrome the panels sit inside, so a panel's breakpoint matches
                // the width it is actually given.
                self.models.update(panels::models::Message::Resized(w - 48.0));
            }
            Message::Models(m) => self.models.update(m),
            Message::Diagnostics(m) => self.diagnostics.update(m),
            Message::Settings(m) => {
                // A save can move the hotkey or change the model, and the dictation worker is the
                // one holding both. It is told here rather than watching the file.
                if self.settings.update(m) {
                    self.dictate.settings_saved();
                }
            }
            Message::Benchmark(m) => {
                return self.benchmark.update(m).map(Message::Benchmark);
            }
            Message::Dictate(m) => {
                self.dictate.update(m);
                // The pump lives with the worker, in the Dictate panel, but the question it answers
                // -- did the OS take this binding? -- is asked on the Settings tab. Copied across
                // here rather than shared, so neither panel can reach into the other.
                self.settings.set_hotkey_status(self.dictate.hotkey_status());
            }
        }
        iced::Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let mut tabs = row![].spacing(4);
        for tab in Tab::ALL {
            let selected = self.tab == tab;
            let label = text(tab.label()).size(15).color(if selected {
                theme::TEXT
            } else {
                theme::TEXT_DIM
            });
            tabs = tabs.push(
                button(label)
                    .padding(Padding::from([6, 14]))
                    .on_press(Message::TabSelected(tab))
                    .style(move |_t, status| tab_style(selected, status)),
            );
        }

        let body: Element<'_, Message> = match self.tab {
            Tab::Dictate => self.dictate.view().map(Message::Dictate),
            Tab::Settings => self.settings.view().map(Message::Settings),
            Tab::Models => self.models.view().map(Message::Models),
            Tab::Benchmark => self.benchmark.view().map(Message::Benchmark),
            Tab::Diagnostics => self.diagnostics.view().map(Message::Diagnostics),
        };

        column![
            container(tabs).padding(Padding {
                top: 8.0,
                right: 12.0,
                bottom: 8.0,
                left: 12.0,
            }),
            container(Space::new(Length::Fill, 1)).style(|_t| container::Style {
                background: Some(theme::BORDER.into()),
                ..Default::default()
            }),
            container(body).padding(16).height(Length::Fill),
        ]
        .into()
    }
}

fn tab_style(selected: bool, status: button::Status) -> button::Style {
    let bg = if selected {
        theme::BG_RAISED
    } else if matches!(status, button::Status::Hovered) {
        theme::faded(theme::BG_RAISED, 0.6)
    } else {
        iced::Color::TRANSPARENT
    };
    button::Style {
        background: Some(bg.into()),
        text_color: if selected { theme::TEXT } else { theme::TEXT_DIM },
        border: iced::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

