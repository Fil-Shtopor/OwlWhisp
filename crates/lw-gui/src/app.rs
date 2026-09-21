//! The windows: the main one -- a tab bar and whichever panel is selected -- and the dictation
//! overlay that comes and goes with a recording.
//!
//! The tab order is the one the web front end settled on -- Dictate, Settings, Models, Benchmark,
//! Diagnostics -- because it is the order of how often a tab is wanted, and changing it would be a
//! change a user can see in a commit that is supposed to change nothing they can see.

use iced::widget::{button, column, container, row, text, Space};
use iced::{Element, Length, Padding, Task, window};

use crate::{panels, smooth, theme, tray};

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
    /// The main window finished opening.
    MainOpened(window::Id),
    /// The overlay finished opening, and can now be made click-through.
    OverlayOpened(window::Id),
    /// A window closed.
    Closed(window::Id),
    /// The main window's close button. Hides to the tray rather than quitting, where there is one.
    CloseRequested(window::Id),
    /// Time to drain the tray's menu channel.
    TrayPoll,
    /// The user picked something in the tray menu.
    Tray(tray::Action),
    /// A wheel notch, taken from the scrollable so it can be animated instead of applied at once.
    Wheel(iced::mouse::ScrollDelta),
    /// What the scrollable reports about itself after every move.
    Scrolled(iced::widget::scrollable::Viewport),
    /// One rendered frame, while a scroll is in flight.
    Frame(std::time::Instant),
}

pub struct App {
    /// The main window, once it exists. A daemon starts with no windows at all.
    main: Option<window::Id>,
    /// The overlay, while a recording is in progress.
    overlay: Option<window::Id>,
    /// The tray icon, if this machine gave us one. Held for its whole life; dropping it removes
    /// the icon.
    tray: Option<tray::Tray>,
    tab: Tab,
    /// The one scrollable in the window, animated rather than stepped.
    scroll: smooth::Scroll,
    models: panels::models::State,
    diagnostics: panels::diagnostics::State,
    settings: panels::settings::State,
    benchmark: panels::benchmark::State,
    dictate: panels::dictate::State,
}

impl Default for App {
    fn default() -> Self {
        Self {
            main: None,
            overlay: None,
            tray: None,
            tab: Tab::Dictate,
            scroll: smooth::Scroll::new(),
            models: panels::models::State::new(),
            diagnostics: panels::diagnostics::State::new(),
            settings: panels::settings::State::new(),
            benchmark: panels::benchmark::State::new(),
            dictate: panels::dictate::State::new(),
        }
    }
}

impl App {
    /// Build the state and open the main window.
    ///
    /// A daemon has no window until one is asked for, which is what makes the overlay possible;
    /// the cost is that the first window has to be opened by hand, here.
    pub fn boot(tray: Option<tray::Tray>) -> (Self, Task<Message>) {
        let (_id, open) = window::open(window::Settings {
            size: iced::Size::new(1000.0, 720.0),
            min_size: Some(iced::Size::new(560.0, 420.0)),
            // The close button is answered by us: with a tray, it hides; without one, it quits.
            exit_on_close_request: false,
            // The same icon as the tray and the Tauri build. Without it the window wears the
            // default winit icon, which is not this application.
            icon: crate::tray::window_icon(),
            ..Default::default()
        });
        let app = Self {
            tray,
            ..Self::default()
        };
        (app, open.map(Message::MainOpened))
    }

    /// Leave, flushing the log first.
    ///
    /// Not `iced::exit()`: once a WebGPU session has existed in this process, ONNX Runtime's
    /// WebGPU execution provider crashes during library detach, so an ordinary exit ends in a
    /// crash dialog rather than a quit. `exit_without_teardown` ends the process outright, which
    /// runs no destructors -- hence the explicit flush.
    fn quit() -> ! {
        tracing::info!("quitting");
        lw_app::logging::flush();
        std::thread::sleep(std::time::Duration::from_millis(80));
        lw_ort::exit_without_teardown(0)
    }

    /// Bring the main window back and put it on `tab`.
    fn show_main(&mut self, tab: Tab) -> Task<Message> {
        self.tab = tab;
        match self.main {
            Some(id) => Task::batch([
                window::change_mode(id, window::Mode::Windowed),
                window::gain_focus(id),
            ]),
            None => Task::none(),
        }
    }

    pub fn title(&self, window: window::Id) -> String {
        if Some(window) == self.overlay {
            // Never seen: the overlay has no decorations and is off the taskbar. Named anyway, so
            // that a tool listing windows shows something a person can identify.
            "LocalWisper Overlay".to_string()
        } else {
            "LocalWisper".to_string()
        }
    }

    pub fn theme(&self, window: window::Id) -> iced::Theme {
        if Some(window) == self.overlay {
            theme::overlay_theme()
        } else {
            theme::theme()
        }
    }

    /// The colour each window is cleared to. See `theme::appearance`.
    pub fn style(&self, theme: &iced::Theme) -> iced::daemon::Appearance {
        theme::appearance(theme)
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
            self.models.subscription().map(Message::Models),
            window::close_events().map(Message::Closed),
            // Frames only while something is moving. A scroll that has arrived costs nothing, and
            // an idle window redraws not at all.
            if self.scroll.animating() {
                iced::time::every(smooth::STEP).map(Message::Frame)
            } else {
                iced::Subscription::none()
            },
            window::close_requests().map(Message::CloseRequested),
            // Only while there is a tray to poll. `tray-icon` publishes on a global channel, so
            // this is a `try_recv` and nothing more.
            if self.tray.is_some() {
                iced::time::every(std::time::Duration::from_millis(200)).map(|_| Message::TrayPoll)
            } else {
                iced::Subscription::none()
            },
        ])
    }

    /// Open or close the overlay window to match the setting -- not the recording.
    ///
    /// The window lives for as long as the user wants an indicator at all, and it is the *pill*
    /// that comes and goes with a recording. That is not how it was first written, and the reason
    /// is worth keeping: a window can only be made non-activating after it exists, so a window
    /// opened at the start of every utterance takes the keyboard focus for the instant before the
    /// style is applied -- which is the instant the user is mid-sentence in another program. Once
    /// open, it is click-through, off the taskbar, off alt-tab, and draws nothing at all while
    /// idle, so a window that is always there is indistinguishable from one that is not.
    ///
    /// Switching the setting off really does close it, which is why this exists at all.
    fn sync_overlay(&mut self) -> Task<Message> {
        match (self.dictate.overlay_enabled(), self.overlay) {
            (true, None) => {
                let area = lw_platform::screen::primary_work_area();
                let position = match area {
                    Some(a) => {
                        let (x, y) = a.bottom_centre(
                            panels::overlay::WIDTH,
                            panels::overlay::HEIGHT,
                            panels::overlay::MARGIN,
                        );
                        window::Position::Specific(iced::Point::new(x, y))
                    }
                    // Rather than guess and risk putting it off-screen.
                    None => window::Position::Centered,
                };

                let (id, open) = window::open(window::Settings {
                    size: iced::Size::new(panels::overlay::WIDTH, panels::overlay::HEIGHT),
                    position,
                    resizable: false,
                    decorations: false,
                    transparent: true,
                    level: window::Level::AlwaysOnTop,
                    exit_on_close_request: false,
                    platform_specific: window::settings::PlatformSpecific {
                        // Off the taskbar: this is an indicator, not a window the user manages.
                        skip_taskbar: true,
                        drag_and_drop: false,
                        undecorated_shadow: false,
                    },
                    ..Default::default()
                });
                self.overlay = Some(id);
                open.map(Message::OverlayOpened)
            }
            (false, Some(id)) => {
                self.overlay = None;
                window::close(id)
            }
            _ => Task::none(),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.route(message);
        Task::batch([task, self.sync_overlay()])
    }

    fn route(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::MainOpened(id) => self.main = Some(id),
            Message::OverlayOpened(id) => {
                // Order matters, and `batch` does not promise any, so these are chained.
                //
                // Click-through goes first because winit rewrites the whole window style from its
                // own bookkeeping when it applies it -- which silently undoes anything set behind
                // its back, including the flag that stops the window taking focus. Ours goes last.
                //
                // The window is left visible as far as winit is concerned, because a window winit
                // believes is hidden is a window it never asks anyone to paint -- which showed up
                // as an overlay that existed, reported itself visible to the OS, and drew nothing.
                let styles = window::enable_mouse_passthrough(id).chain(
                    window::run_with_handle(id, |handle| {
                        if let raw_window_handle::RawWindowHandle::Win32(win32) = handle.as_raw()
                            && let Err(e) = lw_platform::overlay::make_passive(win32.hwnd.get())
                        {
                            tracing::warn!("the overlay may take focus: {e}");
                        }
                    })
                    .discard(),
                );
                // Opening it took the focus, once. Hand it straight back: this only ever happens
                // at startup or when the user switches the indicator on, and in both cases the
                // window they were looking at is ours.
                return match self.main {
                    Some(main) => styles.chain(window::gain_focus(main)),
                    None => styles,
                };
            }
            Message::Closed(id) => {
                if Some(id) == self.overlay {
                    self.overlay = None;
                } else if Some(id) == self.main {
                    self.main = None;
                    // The overlay is not a reason to keep the process alive.
                    Self::quit();
                }
            }
            Message::CloseRequested(id) => {
                if Some(id) != self.main {
                    return window::close(id);
                }
                match self.tray.is_some() {
                    // Hidden, not closed: the application goes on listening for its hotkey, and
                    // the panels keep their state -- an unsaved settings edit survives a stray
                    // click on the close button.
                    true => return window::change_mode(id, window::Mode::Hidden),
                    // With no tray there would be no way back, so the close button means what it
                    // says.
                    false => Self::quit(),
                }
            }
            Message::TrayPoll => {
                let actions = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
                return Task::batch(actions.into_iter().map(|a| Task::done(Message::Tray(a))));
            }
            Message::Tray(action) => {
                return match action {
                    tray::Action::OpenSettings => self.show_main(Tab::Settings),
                    tray::Action::OpenDiagnostics => self.show_main(Tab::Diagnostics),
                    tray::Action::Quit => Self::quit(),
                };
            }
            Message::Wheel(delta) => {
                self.scroll.wheel(delta);
            }
            Message::Scrolled(viewport) => self.scroll.observed(viewport),
            Message::Frame(now) => {
                if let Some(offset) = self.scroll.tick(now) {
                    return iced::widget::scrollable::scroll_to(self.scroll.id(), offset);
                }
            }
            Message::TabSelected(tab) => {
                // Leaving Settings closes the test stream. Holding a microphone open because
                // somebody switched tabs would be indefensible.
                if self.tab == Tab::Settings && tab != Tab::Settings && self.settings.mic_test_on()
                {
                    self.dictate.set_mic_test(false);
                }
                let changed = self.tab != tab;
                self.tab = tab;
                if changed {
                    // The content underneath has been replaced, so the old position means nothing.
                    // Jump rather than slide: sliding through a page nobody has seen is motion
                    // that says nothing.
                    let top = self.scroll.reset();
                    return iced::widget::scrollable::scroll_to(self.scroll.id(), top);
                }
            }
            Message::Resized(w) => {
                // Minus the window chrome the panels sit inside, so a panel's breakpoint matches
                // the width it is actually given.
                self.models.update(panels::models::Message::Resized(w - 48.0));
                self.settings
                    .update(panels::settings::Message::Resized(w - 48.0));
            }
            Message::Models(m) => {
                // Choosing a model writes settings.json, and the worker is holding a copy of it.
                if self.models.update(m) {
                    self.dictate.settings_saved();
                    self.settings = panels::settings::State::new();
                }
            }
            Message::Diagnostics(m) => self.diagnostics.update(m),
            Message::Settings(m) => {
                // The microphone belongs to the worker, which the Dictate panel owns. Forwarded
                // here rather than shared, so neither panel reaches into the other.
                if let panels::settings::Message::MicTestToggled(on) = m {
                    self.dictate.set_mic_test(on);
                }
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
                // Nothing else to do here: `update` reconciles the overlay after every message.
                // The pump lives with the worker, in the Dictate panel, but the question it answers
                // -- did the OS take this binding? -- is asked on the Settings tab. Copied across
                // here rather than shared, so neither panel can reach into the other.
                self.settings.set_hotkey_status(self.dictate.hotkey_status());
                self.settings.set_mic(
                    self.dictate.level(),
                    self.dictate.mic_test(),
                    self.dictate.state() != lw_app::RecordingState::Idle,
                    self.dictate.mic_error(),
                );
            }
        }
        Task::none()
    }

    pub fn view(&self, window: window::Id) -> Element<'_, Message> {
        if Some(window) == self.overlay {
            return panels::overlay::view(self.dictate.state(), self.dictate.phase());
        }
        self.main_view()
    }

    fn main_view(&self) -> Element<'_, Message> {
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
            container(self.scroll.view(body, Message::Wheel, Message::Scrolled))
                .padding(16)
                .height(Length::Fill),
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

