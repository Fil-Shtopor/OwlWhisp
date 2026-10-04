//! Application updates, with background checks and an explicit installation action.
use std::time::{Duration, Instant};

use iced::widget::{button, checkbox, column, progress_bar, row, text};
use iced::{Element, Subscription};
use lw_app::updates::{Available, Downloaded, Job, Preferences};

use crate::{theme, widgets};

#[derive(Clone, Debug)]
pub enum Message {
    Check,
    Poll,
    CheckDue,
    Automatic(bool),
    Previews(bool),
    Download,
    Cancel,
    Install,
    OpenRelease,
    Show,
}

pub struct State {
    preferences: Preferences,
    checking: Option<Job<Option<Available>>>,
    downloading: Option<Job<Downloaded>>,
    ready: Option<Downloaded>,
    available: Option<Available>,
    last_check: Option<Instant>,
    status: String,
    error: Option<String>,
}

impl State {
    pub fn new() -> Self {
        let preferences = Preferences::load();
        let mut state = Self {
            preferences,
            checking: None,
            downloading: None,
            ready: None,
            available: None,
            last_check: None,
            status: "Check for a newer version on GitHub.".into(),
            error: None,
        };
        if state.preferences.automatic_checks {
            state.check();
        }
        state
    }

    pub fn available_version(&self) -> Option<String> {
        self.available.as_ref().map(|a| a.version.to_string())
    }

    fn busy(&self) -> bool {
        self.checking.is_some() || self.downloading.is_some()
    }

    fn check(&mut self) {
        if self.busy() {
            return;
        }
        self.error = None;
        self.status = "Checking GitHub Releases...".into();
        self.last_check = Some(Instant::now());
        self.checking = Some(lw_app::updates::check(
            env!("CARGO_PKG_VERSION"),
            self.preferences.include_previews,
        ));
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            if self.busy() {
                iced::time::every(Duration::from_millis(250)).map(|_| Message::Poll)
            } else {
                Subscription::none()
            },
            if self.preferences.automatic_checks {
                iced::time::every(Duration::from_secs(3600)).map(|_| Message::CheckDue)
            } else {
                Subscription::none()
            },
        ])
    }

    /// Returns true only after a verified installer has successfully started.
    pub fn update(&mut self, message: Message, installation_allowed: bool) -> bool {
        match message {
            Message::Check => self.check(),
            Message::CheckDue => {
                if self
                    .last_check
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(24 * 3600))
                {
                    self.check();
                }
            }
            Message::Automatic(enabled) => {
                self.preferences.automatic_checks = enabled;
                if let Err(error) = self.preferences.save() {
                    self.error = Some(error);
                }
                if enabled {
                    self.check();
                }
            }
            Message::Previews(enabled) => {
                if self.downloading.is_none() {
                    self.preferences.include_previews = enabled;
                    if let Err(error) = self.preferences.save() {
                        self.error = Some(error);
                    }
                    self.checking = None;
                    self.available = None;
                    self.ready = None;
                    self.check();
                }
            }
            Message::Poll => {
                if let Some(result) = self.checking.as_ref().and_then(Job::poll) {
                    self.checking = None;
                    match result {
                        Ok(Some(available)) => {
                            if self
                                .ready
                                .as_ref()
                                .is_some_and(|ready| ready.version != available.version)
                            {
                                self.ready = None;
                            }
                            self.status = format!(
                                "OwlWhisp {} is available{}.",
                                available.version,
                                if available.preview { " (preview)" } else { "" }
                            );
                            self.available = Some(available);
                        }
                        Ok(None) => {
                            self.available = None;
                            self.ready = None;
                            self.status = "You have the newest version for this update channel.".into();
                        }
                        Err(error) => self.error = Some(format!("Could not check for updates: {error}")),
                    }
                }
                if let Some(result) = self.downloading.as_ref().and_then(Job::poll) {
                    self.downloading = None;
                    match result {
                        Ok(ready) => {
                            self.status = format!("OwlWhisp {} is downloaded and verified.", ready.version);
                            self.ready = Some(ready);
                        }
                        Err(error) if error == "Update download cancelled" => {
                            self.status = "Update download cancelled.".into()
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
            }
            Message::Download => {
                if !self.busy()
                    && let Some(available) = &self.available
                {
                    self.error = None;
                    self.ready = None;
                    self.status = "Downloading the update...".into();
                    self.downloading = Some(lw_app::updates::download(available));
                }
            }
            Message::Cancel => {
                if let Some(job) = &self.downloading {
                    job.cancel();
                }
                self.status = "Cancelling the download...".into();
            }
            Message::Install => {
                if !installation_allowed {
                    self.error = Some(
                        "Finish the recording, benchmark or download before installing an update.".into(),
                    );
                } else if let Some(ready) = &self.ready {
                    match lw_app::updates::launch_installer(ready) {
                        Ok(()) => return true,
                        Err(error) => self.error = Some(error),
                    }
                }
            }
            Message::OpenRelease => {
                let url = self
                    .available
                    .as_ref()
                    .map(|a| a.page.as_str())
                    .unwrap_or(lw_app::updates::RELEASES_PAGE);
                if let Err(error) = lw_platform::browser::open_https(url) {
                    self.error = Some(error.to_string());
                }
            }
            Message::Show => {}
        }
        false
    }

    pub fn banner(&self) -> Option<Element<'_, Message>> {
        let available = self.available.as_ref()?;
        Some(
            row![
                text(format!("OwlWhisp {} is available", available.version))
                    .size(14)
                    .color(theme::ACCENT),
                button("View update").on_press(Message::Show),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center)
            .into(),
        )
    }

    pub fn view(&self, installation_allowed: bool) -> Element<'_, Message> {
        let mut body = column![
            widgets::heading("Application updates"),
            widgets::sub(format!("Installed version: {}", env!("CARGO_PKG_VERSION"))),
            checkbox(self.preferences.automatic_checks).label("Check automatically at startup and once a day").on_toggle(Message::Automatic),
            checkbox(self.preferences.include_previews).label("Include preview releases").on_toggle(Message::Previews),
            widgets::prose("Contacts GitHub for version information. Your audio, models and settings stay on your device."),
            widgets::sub(&self.status),
        ].spacing(10);
        let mut actions =
            row![button("Check for updates").on_press_maybe((!self.busy()).then_some(Message::Check))]
                .spacing(8);
        if let Some(available) = &self.available {
            if let Some(job) = &self.downloading {
                let (done, total) = job.progress();
                body = body.push(progress_bar(0.0..=total.max(1) as f32, done as f32));
                body = body.push(widgets::sub(format!(
                    "{:.1} / {:.1} MiB",
                    done as f64 / 1048576.0,
                    total as f64 / 1048576.0
                )));
                actions = actions.push(button("Cancel download").on_press(Message::Cancel));
            } else if self.ready.is_some() {
                actions = actions.push(
                    button("Install update").on_press_maybe(installation_allowed.then_some(Message::Install)),
                );
                body = body.push(widgets::prose("Closes OwlWhisp and opens the installer. Your downloaded models and settings are preserved."));
                if !installation_allowed {
                    body = body.push(widgets::sub("Finish the current task before installing."));
                }
            } else if available.can_install() && lw_app::updates::packaged_windows_app() {
                actions = actions.push(
                    button("Download update").on_press_maybe((!self.busy()).then_some(Message::Download)),
                );
            }
            actions = actions.push(button("Release notes").on_press(Message::OpenRelease));
        }
        body = body.push(actions);
        if let Some(error) = &self.error {
            body = body.push(text(error).size(13).color(theme::BAD));
        }
        widgets::card(body).into()
    }
}
