//! The Models tab.
//!
//! A port of `app/frontend/src/panels/Models.tsx`, structure for structure: the machine card, the
//! models root with a refresh, the estimate disclaimer, the role picker, the glossary, and the
//! table with its expandable rows. Every decision it used to make in TypeScript -- which model to
//! suggest for a role and a language, which accelerator can run what, how to group by maker --
//! now comes from `lw_app`, so this file only draws.

use std::collections::BTreeSet;
use std::fmt;

use iced::widget::{Space, button, column, container, pick_list, row};
use iced::{Element, Length, Padding};
use lw_app::catalog::{CatalogView, EntryView};
use lw_core::model::{InstallState, ModelRole};

use crate::{theme, widgets};

/// One entry in the language picker. A newtype so it can carry the code while showing the name.
#[derive(Clone, PartialEq, Eq)]
pub struct Lang {
    code: String,
    name: String,
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Refresh,
    Resized(f32),
    ToggleRow(String),
    ExpandAll,
    CollapseAll,
    LanguageSelected(String),
    GlossaryToggled,
    AskDelete(String),
    CancelDelete,
    /// Start (or resume) downloading a model.
    Install(String),
    /// Ask the download in flight to stop. Partial files stay, so it resumes later.
    CancelInstall,
    /// Drain the download's progress channel.
    InstallTick,
    /// Make a model the one dictation uses.
    Select(String),
    /// Remove a model's files, having been confirmed.
    Delete(String),
}

pub struct State {
    /// The window's width, kept up to date by the application from resize events. Used for the
    /// column breakpoints; `responsive` cannot supply it inside a scrollable.
    width: f32,
    catalog: Result<CatalogView, String>,
    mine: std::collections::BTreeMap<String, Vec<lw_app::LocalMeasurement>>,
    open: BTreeSet<String>,
    language: String,
    glossary: bool,
    confirm_delete: Option<String>,
    languages: Vec<Lang>,
    /// The download in flight, if any. One at a time: two concurrent multi-gigabyte downloads
    /// share one link and both finish later than they would in sequence.
    install: Option<lw_app::install::Handle>,
    /// The model dictation is set to use, re-read whenever it might have changed.
    selected: String,
    /// The last thing that happened, good or bad, shown where the buttons are.
    notice: Option<String>,
    error: Option<String>,
}

impl State {
    pub fn new() -> Self {
        let mut s = Self {
            width: 1000.0,
            catalog: Err("not loaded".into()),
            mine: Default::default(),
            open: Default::default(),
            language: String::new(),
            glossary: false,
            confirm_delete: None,
            languages: Vec::new(),
            install: None,
            selected: String::new(),
            notice: None,
            error: None,
        };
        // A fresh install has no settings file yet. Persist the defaults before starting the
        // background download, both to make the selected Parakeet model explicit and to ensure a
        // restart resumes rather than starts a second first-run download.
        let settings_path = lw_app::paths::settings_path();
        let first_run = !settings_path.exists();
        s.load();
        if first_run {
            if let Err(e) = lw_core::settings::Settings::default().save(&settings_path) {
                tracing::warn!("could not save first-run settings: {e}");
            } else {
                s.start_default_download();
            }
        }
        s
    }

    /// Fetch Parakeet once for a newly installed application. This deliberately happens after
    /// the first window exists, rather than inside the installer: downloads can be resumed,
    /// cancelled, and reported in the Models tab instead of leaving an installer apparently hung
    /// on a 600+ MiB network operation.
    fn start_default_download(&mut self) {
        const DEFAULT_MODEL: &str = "parakeet-tdt-0.6b-v3";
        let missing = self.catalog.as_ref().is_ok_and(|catalog| {
            catalog
                .entries
                .iter()
                .find(|entry| entry.id == DEFAULT_MODEL)
                .is_some_and(|entry| {
                    matches!(
                        entry.install_state,
                        lw_core::model::InstallState::Missing | lw_core::model::InstallState::Incomplete
                    )
                })
        });
        if missing {
            self.notice = Some("Downloading the default Parakeet model…".into());
            self.install = Some(lw_app::install::start(
                &lw_app::paths::settings_path(),
                DEFAULT_MODEL,
            ));
        } else if self.catalog.is_err() {
            self.error = Some(
                "The default model could not be prepared because its bundled download manifest is missing. Reinstall OwlWhisp.".into(),
            );
        }
    }

    fn load(&mut self) {
        // The app's own directory, not `default_models_root`: that is where the Tauri build
        // installed everything, and looking anywhere else reports an empty catalog.
        self.catalog = lw_app::catalog::build(&lw_app::paths::models_root());
        let path = lw_app::measurements::path_for(&lw_app::paths::settings_path());
        self.mine = lw_app::LocalMeasurements::load(&path).models;
        self.languages = self.build_languages();
        self.selected = lw_core::settings::Settings::load(&lw_app::paths::settings_path())
            .map(|s| s.model_id)
            .unwrap_or_default();
    }

    /// Redraw while a download is running, so the bar moves.
    pub fn subscription(&self) -> iced::Subscription<Message> {
        if self.install.is_some() {
            iced::time::every(std::time::Duration::from_millis(200)).map(|_| Message::InstallTick)
        } else {
            iced::Subscription::none()
        }
    }

    /// Sorted by name, not by code: the list is read alphabetically by a human looking for theirs,
    /// and by code "Ukrainian" sits between "tt" and "ur".
    fn build_languages(&self) -> Vec<Lang> {
        let Ok(view) = &self.catalog else {
            return Vec::new();
        };
        let mut out: Vec<Lang> = vec![Lang {
            code: String::new(),
            name: "Any".into(),
        }];
        for e in &view.entries {
            for (i, code) in e.languages.iter().enumerate() {
                if out.iter().any(|l| l.code == *code) {
                    continue;
                }
                out.push(Lang {
                    code: code.clone(),
                    name: e.language_names.get(i).cloned().unwrap_or_else(|| code.clone()),
                });
            }
        }
        out[1..].sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    fn language_name(&self, code: &str) -> String {
        self.languages
            .iter()
            .find(|l| l.code == code)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| code.to_string())
    }

    /// Apply one message; the answer is whether `settings.json` was written, because the model
    /// dictation uses is in it and the worker is holding a copy.
    pub fn update(&mut self, message: Message) -> bool {
        match message {
            Message::Refresh => self.load(),
            Message::Resized(w) => self.width = w,
            Message::ToggleRow(id) => {
                if !self.open.remove(&id) {
                    self.open.insert(id);
                }
            }
            Message::ExpandAll => {
                if let Ok(v) = &self.catalog {
                    self.open = v.entries.iter().map(|e| e.id.clone()).collect();
                }
            }
            Message::CollapseAll => self.open.clear(),
            Message::LanguageSelected(code) => self.language = code,
            Message::GlossaryToggled => self.glossary = !self.glossary,
            Message::AskDelete(id) => self.confirm_delete = Some(id),
            Message::CancelDelete => self.confirm_delete = None,

            Message::Install(id) => {
                if self.install.is_some() {
                    return false;
                }
                self.notice = None;
                self.error = None;
                self.install = Some(lw_app::install::start(&lw_app::paths::settings_path(), &id));
            }
            Message::CancelInstall => {
                if let Some(h) = &self.install {
                    h.cancel();
                }
            }
            Message::InstallTick => {
                let Some(handle) = &self.install else {
                    return false;
                };
                let Some(outcome) = handle.poll() else {
                    return false;
                };
                let id = handle.id.clone();
                self.install = None;
                match outcome {
                    lw_app::install::Progress::Done { .. } => {
                        self.notice = Some(format!("{id} downloaded."));
                        // The catalog reads the disk to decide what is installed, so it has to be
                        // rebuilt before the row can stop offering a download.
                        self.load();
                    }
                    lw_app::install::Progress::Cancelled => {
                        self.notice = Some(format!(
                            "{id}: download stopped. What was fetched is kept, so starting again \
                             resumes."
                        ));
                        self.load();
                    }
                    lw_app::install::Progress::Failed { message } => {
                        self.error = Some(format!("{id}: {message}"));
                        self.load();
                    }
                    _ => {}
                }
            }
            Message::Select(id) => {
                self.notice = None;
                self.error = None;
                match lw_app::install::select(&lw_app::paths::settings_path(), &id) {
                    Ok(()) => {
                        self.selected = id;
                        self.notice = Some("Dictation will use this model.".into());
                        return true;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Message::Delete(id) => {
                self.confirm_delete = None;
                self.notice = None;
                self.error = None;
                match lw_app::install::delete(&lw_app::paths::settings_path(), &id) {
                    Ok(freed) => {
                        self.notice = Some(format!("Deleted {id}, freeing {}.", format_bytes(freed)));
                        self.load();
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        false
    }

    pub fn view(&self) -> Element<'_, Message> {
        let view = match &self.catalog {
            Ok(v) => v,
            Err(e) => {
                return widgets::card(
                    column![
                        widgets::heading("The catalog could not be read"),
                        widgets::prose(e.clone()),
                    ]
                    .spacing(6),
                )
                .into();
            }
        };

        column![
            machine_card(view),
            row![
                button(widgets::button_label("Refresh"))
                    .padding(Padding::from([6, 14]))
                    .on_press(Message::Refresh)
                    .style(theme::action(false)),
                widgets::mono(view.models_root.clone()),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center),
            disclaimer(view.estimate_disclaimer),
            self.role_picks(view),
            self.glossary_block(view),
            self.table(view),
        ]
        .spacing(12)
        .padding(Padding::from([0, 8]))
        .into()
    }

    fn role_picks<'a>(&'a self, view: &'a CatalogView) -> Element<'a, Message> {
        let measured = lw_app::catalog::measured_languages(&view.entries);
        let selected = self.languages.iter().find(|l| l.code == self.language).cloned();

        let head = row![
            column![
                widgets::heading("Which one should you use?"),
                widgets::sub("A suggestion, not a switch - nothing here changes which model runs."),
            ]
            .spacing(2),
            Space::new().width(Length::Fill),
            widgets::sub("Language"),
            pick_list(self.languages.clone(), selected, |l: Lang| {
                Message::LanguageSelected(l.code)
            })
            .text_size(14),
        ]
        .spacing(10)
        .width(Length::Fill)
        .align_y(iced::Alignment::Center);

        let mut body = column![head].spacing(10);

        for role in ModelRole::ALL {
            let mut block = column![
                row![
                    widgets::badge(role.label(), theme::TEXT_DIM),
                    widgets::sub(role.blurb()),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
            ]
            .spacing(2);

            match lw_app::catalog::pick_for_role(&view.entries, *role, &self.language) {
                None => {
                    block = block.push(widgets::sub(if self.language.is_empty() {
                        "Nothing in this catalog fills that role.".to_string()
                    } else {
                        format!(
                            "Nothing here fills that role for {}.",
                            self.language_name(&self.language)
                        )
                    }));
                }
                Some(pick) => {
                    block = block.push(
                        iced::widget::text(pick.name.clone())
                            .size(14)
                            .color(theme::ACCENT),
                    );
                    match pick.measured_for_language(&self.language) {
                        Some(rate) => {
                            block = block.push(widgets::sub(format!(
                                "{} {:.1}% measured on {} {} clip{}",
                                if rate.unit == lw_core::bench::ErrorUnit::Character {
                                    "CER"
                                } else {
                                    "WER"
                                },
                                rate.rate * 100.0,
                                rate.clips,
                                self.language_name(&self.language),
                                if rate.clips == 1 { "" } else { "s" },
                            )));
                        }
                        None if !self.language.is_empty() && !measured.contains(&self.language) => {
                            // The honest version of a missing number: without this the pick looks
                            // equally well-founded whether it rests on a measurement of the chosen
                            // language or on a figure from four entirely different ones.
                            block = block.push(widgets::sub(format!(
                                "Not measured: there are no {} fixtures. Ranked on this model's \
                                 overall figure, which comes from other languages.",
                                self.language_name(&self.language)
                            )));
                        }
                        None => {}
                    }
                }
            }
            body = body.push(block);
        }

        body = body.push(widgets::prose(
            "The roles themselves are editorial, but the pick within each one is not: it uses that \
             role's own criterion - fewest errors, least delay, smallest download, most languages. \
             For accuracy with a language chosen it uses the rate measured on that language, \
             because every candidate was scored on the same clips. On Any there is no such \
             comparison to make: a blended rate is not one measurement, so those are compared \
             coarsely and the tie goes to the model covering more languages.",
        ));

        widgets::card(body).into()
    }

    fn glossary_block<'a>(&'a self, _view: &'a CatalogView) -> Element<'a, Message> {
        let head = button(
            row![
                widgets::sub(if self.glossary { "v" } else { ">" }),
                widgets::body("What the numbers mean, and where they come from"),
            ]
            .spacing(8),
        )
        .padding(Padding::from([6, 8]))
        .on_press(Message::GlossaryToggled)
        .style(|_t, _s| button::Style {
            background: None,
            text_color: theme::TEXT,
            ..Default::default()
        });

        let mut block = column![head].spacing(6);
        if self.glossary {
            block = block
                .push(widgets::prose(
                    "RTF and Accuracy show the catalog's ASUS Zenbook A16 measurements. Parakeet \
                     uses its NPU run. Benchmarks on this computer appear only under On your machine.",
                ))
                .push(widgets::prose(
                    "The Fast recommendation still uses an estimated speed tier. It does not \
                     change the measured RTF in the table.",
                ))
                .push(widgets::prose(
                    "WER is the word error rate and CER the character error rate: the share of \
                     words, or of characters for languages written without spaces, that came out \
                     wrong. Lower is better in both. RTF is the real-time factor, seconds of \
                     computing per second of audio. Lower is faster.",
                ));
        }
        widgets::card(block).into()
    }

    fn table<'a>(&'a self, view: &'a CatalogView) -> Element<'a, Message> {
        let all_open = view.entries.iter().all(|e| self.open.contains(&e.id));
        let toolbar = row![
            button(widgets::button_label(if all_open {
                "Collapse all"
            } else {
                "Expand all"
            }))
            .padding(Padding::from([6, 14]))
            .on_press(if all_open {
                Message::CollapseAll
            } else {
                Message::ExpandAll
            })
            .style(theme::action(false)),
            Space::new().width(Length::Fill),
            widgets::sub(format!("{} models", view.entries.len())),
        ]
        .width(Length::Fill)
        .align_y(iced::Alignment::Center);

        // The width comes from the window rather than from `responsive`, which measures its
        // parent and therefore reports nothing useful inside a scrollable.
        let table = self.table_body(view, self.width);

        widgets::card(column![toolbar, table].spacing(8)).into()
    }

    fn table_body<'a>(&'a self, view: &'a CatalogView, width: f32) -> Element<'a, Message> {
        let cols = columns_for(width);
        let mut body = column![header(&cols)].spacing(0);
        body = body.push(iced::widget::rule::horizontal(1).style(theme::rule));

        for (vendor, rows) in lw_app::catalog::group_by_vendor(&view.entries, view.recommended.as_deref()) {
            body = body.push(Space::new().height(8));
            body = body.push(
                row![
                    iced::widget::text(vendor.to_uppercase())
                        .size(11)
                        .color(theme::TEXT),
                    widgets::sub(format!(
                        "{} model{}",
                        rows.len(),
                        if rows.len() == 1 { "" } else { "s" }
                    )),
                ]
                .spacing(8),
            );
            for entry in rows {
                body = body.push(self.row(view, entry, &cols));
                body = body.push(iced::widget::rule::horizontal(1).style(theme::rule));
            }
        }

        body = body.push(Space::new().height(8));
        body = body.push(widgets::prose(
            "Models are grouped by who made them: the maker of the recommended model first, then \
             the makers offering the most, with anything uncredited under Other. Speed is an \
             estimate computed from the model's speed tier and this machine, never a measurement, \
             and the tilde marks it. Under Accuracy the tier is an editorial ranking of the model \
             family, while a green error rate is a real measurement and the chip beside it says \
             which accelerator it was taken on. Open a row for the full detail and the buttons.",
        ));
        body.into()
    }

    fn row<'a>(
        &'a self,
        view: &'a CatalogView,
        entry: &'a EntryView,
        // Its own lifetime: the returned element borrows `self` and `entry`, never the column
        // list, which is rebuilt every frame from the current width.
        cols: &[Col],
    ) -> Element<'a, Message> {
        let open = self.open.contains(&entry.id);
        let recommended = view.recommended.as_deref() == Some(entry.id.as_str());
        static EMPTY: &[lw_app::LocalMeasurement] = &[];
        let mine: &[lw_app::LocalMeasurement] =
            self.mine.get(&entry.id).map(|v| v.as_slice()).unwrap_or(EMPTY);

        // The name and its badges get separate, bounded cells. Sharing one cell went wrong twice:
        // in the web build the badges were unshrinkable and the name collapsed to "S", and here
        // the row divided the space evenly and wrapped both, a letter per line.
        let name_cell = row![
            iced::widget::text(if open { "v" } else { ">" })
                .size(11)
                .color(theme::TEXT_DIM),
            iced::widget::text(entry.name.clone())
                .size(14)
                .color(theme::TEXT)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        let mut badges = row![].spacing(4).align_y(iced::Alignment::Center);
        if recommended {
            badges = badges.push(widgets::badge("* Recommended", theme::ACCENT));
        }
        // Role pills only when there is room for them whole. Chasing them into a narrow cell
        // produced a pill reading "F / a / s / t", which looks like a rendering fault rather than
        // a full cell -- and the roles are already spelled out in the picker above and in the
        // expansion, so shedding them here loses nothing. The web table shed columns at narrow
        // widths for the same reason.
        if self.width >= 1200.0 || !recommended {
            for r in &entry.roles {
                badges = badges.push(widgets::badge(r.label, theme::TEXT_DIM));
            }
        }

        let mut line = row![
            container(name_cell)
                .width(Length::FillPortion(NAME_PORTION))
                .clip(true),
            // The inner fixed width is what stops the wrapping. A row squeezes Shrink children
            // when they do not fit, and a squeezed pill wraps a letter per line however firmly its
            // text says not to; given a width it cannot be squeezed below, the outer container
            // clips the overflow cleanly instead.
            container(container(badges).width(Length::Fixed(400.0)))
                .width(Length::FillPortion(BADGE_PORTION))
                .clip(true),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        for col in cols {
            line = line.push(
                container(col.cell(entry, mine, self.width >= 1200.0))
                    .width(Length::FillPortion(col.portion()))
                    .clip(true),
            );
        }

        let head = button(line)
            .padding(Padding::from([8, 4]))
            .width(Length::Fill)
            .on_press(Message::ToggleRow(entry.id.clone()))
            .style(|_t, status| button::Style {
                background: match status {
                    button::Status::Hovered => Some(theme::faded(theme::BG_INSET, 0.6).into()),
                    _ => None,
                },
                text_color: theme::TEXT,
                border: iced::Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        if !open {
            return head.into();
        }
        column![head, self.expansion(entry, mine)].spacing(6).into()
    }

    fn expansion<'a>(
        &'a self,
        entry: &'a EntryView,
        mine: &'a [lw_app::LocalMeasurement],
    ) -> Element<'a, Message> {
        let mut body = column![widgets::prose(entry.description.clone())].spacing(6);

        body = body.push(fact("Languages", entry.language_names.join(", ")));
        body = body.push(fact("Licence", entry.licence.clone()));
        if let Some(b) = entry.download_bytes {
            body = body.push(fact("Download", format_bytes(b)));
        }
        if let Some(b) = entry.disk_bytes {
            body = body.push(fact("On disk", format_bytes(b)));
        }
        body = body.push(fact("Quality", entry.quality_label.to_string()));
        body = body.push(fact("Speed", entry.speed_label.to_string()));

        body = body.push(Space::new().height(6));
        body = body.push(accelerator_table(entry, mine));

        if let Some(dir) = &entry.install_dir {
            body = body.push(widgets::sub(dir.clone()));
        }
        if let Some(url) = &entry.upstream_url {
            body = body.push(widgets::sub(format!("Upstream: {url}")));
        }

        body = body.push(self.actions(entry));

        widgets::inset(body).into()
    }

    /// Download, use, delete -- and the reason a button is unavailable, on the button itself.
    ///
    /// A disabled control with no explanation is the worst of both: it says no and not why. So
    /// each one that cannot be pressed carries a line underneath saying what would make it
    /// pressable.
    fn actions<'a>(&'a self, entry: &'a EntryView) -> Element<'a, Message> {
        use lw_core::model::InstallState as Install;

        let installed = entry.install_state == Install::Installed;
        let is_selected = self.selected == entry.id;
        let installing = self.install.as_ref().is_some_and(|h| h.id == entry.id);
        let other_installing = self.install.is_some() && !installing;

        let mut buttons = row![].spacing(8).align_y(iced::Alignment::Center);
        let mut why: Vec<String> = Vec::new();

        // --- download ------------------------------------------------------
        if installing {
            let state = self.install.as_ref().map(|h| h.state()).unwrap_or_default();
            buttons = buttons.push(
                button(widgets::button_label(if state.cancelling {
                    "Stopping..."
                } else {
                    "Cancel download"
                }))
                .padding(Padding::from([6, 12]))
                .style(theme::action(false))
                .on_press_maybe((!state.cancelling && !state.finishing).then_some(Message::CancelInstall)),
            );
        } else {
            let can = match entry.install_state {
                Install::Missing | Install::Incomplete => entry.runnable,
                _ => false,
            };
            if !can {
                why.push(match entry.install_state {
                    Install::Installed => "Already downloaded.".into(),
                    Install::Unpinned => "No pinned manifest yet, so there is nothing to verify a download \
                         against - this one cannot be fetched from here."
                        .into(),
                    _ => "This build cannot run the model on this machine, so downloading it \
                          would not help."
                        .to_string(),
                });
            }
            if other_installing {
                why.push("Another download is running; one at a time.".into());
            }
            buttons = buttons.push(
                button(widgets::button_label(match entry.install_state {
                    Install::Incomplete => "Resume download",
                    _ => "Download",
                }))
                .padding(Padding::from([6, 12]))
                .style(theme::action(false))
                .on_press_maybe((can && !other_installing).then(|| Message::Install(entry.id.clone()))),
            );
        }

        // --- use -----------------------------------------------------------
        buttons = buttons.push(
            button(widgets::button_label(if is_selected {
                "In use"
            } else {
                "Use this model"
            }))
            .padding(Padding::from([6, 12]))
            .style(theme::action(false))
            .on_press_maybe(
                (installed && entry.runnable && !is_selected).then(|| Message::Select(entry.id.clone())),
            ),
        );
        if !is_selected && installed && !entry.runnable {
            why.push("This build cannot run it on this machine.".into());
        } else if !is_selected && !installed {
            why.push("Download it before it can be used.".into());
        }

        // --- delete ----------------------------------------------------------
        let deletable = matches!(entry.install_state, Install::Installed | Install::Incomplete);
        if deletable {
            let confirming = self.confirm_delete.as_deref() == Some(entry.id.as_str());
            if confirming {
                // Irreversible, so the confirmation sits inline where the row is rather than over
                // the whole window, it names what it frees, and backing out is the easier of the
                // two.
                buttons = buttons.push(widgets::sub(format!(
                    "Delete {}'s files{}?",
                    entry.name,
                    entry
                        .disk_bytes
                        .map(|b| format!(", freeing {}", format_bytes(b)))
                        .unwrap_or_default()
                )));
                buttons = buttons.push(
                    button(widgets::button_label("Keep"))
                        .padding(Padding::from([6, 12]))
                        .style(theme::action(false))
                        .on_press(Message::CancelDelete),
                );
                buttons = buttons.push(
                    button(widgets::button_label("Delete"))
                        .padding(Padding::from([6, 12]))
                        .style(theme::action(true))
                        .on_press(Message::Delete(entry.id.clone())),
                );
            } else {
                buttons = buttons.push(
                    button(widgets::button_label("Delete"))
                        .padding(Padding::from([6, 12]))
                        .style(theme::action(true))
                        .on_press_maybe(
                            (!is_selected && !installing).then(|| Message::AskDelete(entry.id.clone())),
                        ),
                );
                if is_selected {
                    why.push("This is the model dictation uses. Choose another one first.".into());
                }
            }
        }

        let mut block = column![buttons].spacing(4);

        if installing {
            block = block.push(self.install_progress());
        }
        for line in why {
            block = block.push(widgets::sub(line));
        }
        if let Some(n) = &self.notice {
            block = block.push(widgets::sub(n.clone()));
        }
        if let Some(e) = &self.error {
            block = block.push(iced::widget::text(e.clone()).size(13).color(theme::BAD));
        }
        block.into()
    }

    /// What the download is doing, in bytes rather than in a spinner.
    fn install_progress(&self) -> Element<'_, Message> {
        let Some(handle) = &self.install else {
            return Space::new().into();
        };
        let s = handle.state();
        if s.finishing {
            return widgets::sub(
                "Verified. Moving the files into place - this can take a moment on a large model.",
            )
            .into();
        }
        let head = if s.file_count > 0 {
            format!("File {} of {}: {}", s.file_index + 1, s.file_count, s.file)
        } else {
            "Starting...".to_string()
        };
        let bytes = match s.fraction() {
            Some(f) => format!(
                "{} of {} ({:.0}%)",
                format_bytes(s.received),
                format_bytes(s.total),
                f * 100.0
            ),
            None => format_bytes(s.received),
        };
        column![widgets::sub(head), widgets::mono(bytes)]
            .spacing(2)
            .into()
    }
}

fn machine_card(view: &CatalogView) -> Element<'_, Message> {
    widgets::card(
        column![
            iced::widget::text("THIS MACHINE").size(11).color(theme::TEXT_DIM),
            widgets::mono(view.machine.clone()),
            widgets::sub("RTF and accuracy below use the ASUS Zenbook A16 reference runs."),
        ]
        .spacing(4),
    )
    .into()
}

fn disclaimer(text: &str) -> Element<'_, Message> {
    // A left rule in the accent colour, as the stylesheet has it: this is the one piece of prose
    // in the tab that must not be skimmed past.
    //
    // Two nested containers rather than a `Fill`-height spacer beside the text. Inside a
    // scrollable there is no bounded height for `Fill` to mean anything, so it resolved to the
    // whole viewport: the disclaimer filled the window and everything below it -- the role picker,
    // the glossary, the entire table -- was pushed off the bottom. The stripe is the outer
    // container's left padding showing through instead, which needs no height at all.
    container(
        container(widgets::prose(text))
            .padding(12)
            .width(Length::Fill)
            .style(theme::inset),
    )
    .padding(Padding {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 3.0,
    })
    .style(|_t| container::Style {
        background: Some(theme::ACCENT.into()),
        border: iced::Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

fn fact(label: &str, value: String) -> Element<'_, Message> {
    row![
        container(iced::widget::text(label).size(11).color(theme::TEXT_DIM)).width(90),
        widgets::body(value),
    ]
    .spacing(8)
    .into()
}

fn accelerator_table<'a>(entry: &'a EntryView, mine: &'a [lw_app::LocalMeasurement]) -> Element<'a, Message> {
    // Rows only for accelerators there is something to say about; the rest are summarised under
    // the table grouped by reason, because a sherpa entry refuses eight of them for one reason and
    // eight rows repeating one sentence is worse than the chips this replaced.
    let has_number = |a: &lw_app::catalog::AcceleratorView| {
        entry
            .measurements
            .iter()
            .any(|m| same_accel(m.hardware.label(), a.id))
            || mine.iter().any(|m| same_accel(&m.accelerator, a.id))
    };

    let mut table = column![
        row![
            container(iced::widget::text("Accelerator").size(11).color(theme::TEXT_DIM))
                .width(Length::FillPortion(3)),
            container(
                iced::widget::text("Model supports")
                    .size(11)
                    .color(theme::TEXT_DIM)
            )
            .width(Length::FillPortion(3)),
            container(
                iced::widget::text("On your machine")
                    .size(11)
                    .color(theme::TEXT_DIM)
            )
            .width(Length::FillPortion(4)),
            container(
                iced::widget::text("In the catalog")
                    .size(11)
                    .color(theme::TEXT_DIM)
            )
            .width(Length::FillPortion(4)),
        ]
        .spacing(8)
    ]
    .spacing(4);

    for a in entry.accelerators.iter().filter(|a| a.supported || has_number(a)) {
        // A "no" carries its reason in the cell. The badge alone says the row is off and not
        // what would turn it on, which is the only part a reader can act on.
        let runs: Element<'_, Message> = if a.supported {
            widgets::badge_yes("yes")
        } else {
            column![
                widgets::badge_no("no"),
                widgets::sub(a.reason.clone().unwrap_or_else(|| "not supported".into())),
            ]
            .spacing(2)
            .into()
        };
        // Local benchmark records and the fixed A16 catalog runs stay in separate columns.
        let here: Element<'_, Message> = match mine.iter().find(|m| same_accel(&m.accelerator, a.id)) {
            Some(local) => {
                let mut c = column![widgets::mono(format!(
                    "RTF {:.4}",
                    local.warm_rtf.unwrap_or(local.cold_rtf)
                ))]
                .spacing(2);
                for u in &local.by_unit {
                    c = c.push(widgets::measured(
                        if u.unit == lw_core::bench::ErrorUnit::Character {
                            "CER"
                        } else {
                            "WER"
                        },
                        u.rate,
                    ));
                }
                // What the figure covers, and when. A rate over three clips is not a rate over
                // twelve, and a measurement from a month ago is not a measurement of today's
                // build -- neither is visible from the number alone.
                c = c.push(widgets::sub(local_detail(local)));
                c.into()
            }
            None => widgets::sub(if a.supported { "not measured yet" } else { "-" }).into(),
        };
        let there: Element<'_, Message> = match entry
            .measurements
            .iter()
            .find(|m| same_accel(m.hardware.label(), a.id))
        {
            Some(m) => {
                let mut r = row![widgets::mono(format!("RTF {:.4}", m.rtf))].spacing(6);
                if let Some(w) = m.wer {
                    r = r.push(widgets::measured("WER", w));
                }
                // The machine it was taken on, because a catalog figure is somebody else's
                // measurement and means nothing without one -- but only the machine. `source` is
                // the full methodology paragraph, and putting it in a column this narrow buried
                // the row's own buttons under six hundred pixels of prose. It lives in the
                // tooltip, which is where the web version kept both of them.
                iced::widget::tooltip(
                    column![r, widgets::sub(short_machine(&m.machine))].spacing(2),
                    widgets::inset(widgets::prose(format!(
                        "{}

{}",
                        m.machine, m.source
                    ))),
                    iced::widget::tooltip::Position::FollowCursor,
                )
                .into()
            }
            None => widgets::sub("-").into(),
        };

        table = table.push(
            row![
                container(widgets::body(a.label)).width(Length::FillPortion(3)),
                container(runs).width(Length::FillPortion(3)),
                container(here).width(Length::FillPortion(4)),
                container(there).width(Length::FillPortion(4)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Start),
        );
    }

    let mut by_reason: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for a in entry
        .accelerators
        .iter()
        .filter(|a| !a.supported && !has_number(a))
    {
        by_reason
            .entry(a.reason.as_deref().unwrap_or("not supported"))
            .or_default()
            .push(a.label);
    }
    for (reason, labels) in by_reason {
        table = table.push(
            row![
                widgets::badge_no("cannot run"),
                widgets::sub(format!("{} - {reason}.", labels.join(", "))),
            ]
            .spacing(8),
        );
    }

    table = table.push(widgets::prose(
        "\u{201c}Model supports\u{201d} describes the model and engine, not whether this package \
         has the provider or this PC has the hardware. Settings shows actual readiness. \
         \u{201c}On your machine\u{201d} is empty until you run a benchmark; \
         \u{201c}in the catalog\u{201d} contains the ASUS Zenbook A16 reference runs.",
    ));

    // A run of ours filed under an accelerator this build does not recognise. Listed rather than
    // dropped: it is a real measurement, and silently hiding it is the one thing this table
    // exists to prevent. Records written before the sherpa engine reported an accelerator id land
    // here, filed under "unknown".
    let orphans: Vec<&lw_app::LocalMeasurement> = mine
        .iter()
        .filter(|m| {
            !entry
                .accelerators
                .iter()
                .any(|a| same_accel(&m.accelerator, a.id))
        })
        .collect();
    if !orphans.is_empty() {
        let list = orphans
            .iter()
            .map(|m| {
                format!(
                    "{}: RTF {:.4}{}",
                    m.accelerator_label,
                    m.warm_rtf.unwrap_or(m.cold_rtf),
                    m.wer
                        .map(|w| format!(" \u{b7} WER {:.1}%", w * 100.0))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        table = table.push(widgets::prose(format!(
            "Measured here, but filed under an accelerator this build does not recognise - older \
             runs recorded the backend but not which accelerator it was: {list}. Re-run the \
             benchmark to file them properly.",
        )));
    }

    // The same on the catalog side: a figure on hardware this build has no accelerator for.
    let unmatched: Vec<&lw_core::model::MeasuredPoint> = entry
        .measurements
        .iter()
        .filter(|m| {
            !entry
                .accelerators
                .iter()
                .any(|a| same_accel(m.hardware.label(), a.id))
        })
        .collect();
    if !unmatched.is_empty() {
        let list = unmatched
            .iter()
            .map(|m| {
                format!(
                    "{}: RTF {:.4}{}",
                    m.hardware.label(),
                    m.rtf,
                    m.wer
                        .map(|w| format!(" \u{b7} WER {:.1}%", w * 100.0))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        table = table.push(widgets::prose(format!(
            "Also in the catalog, on hardware this build has no accelerator for: {list}.",
        )));
    }

    table.into()
}

/// What a local measurement covers, in one line: how many clips, which languages, what was left
/// out, and when it was taken.
fn local_detail(m: &lw_app::LocalMeasurement) -> String {
    let mut parts: Vec<String> = Vec::new();

    if m.by_unit.len() > 1 {
        // Two units do not average, so the coverage is stated per unit rather than blended.
        parts.push(
            m.by_unit
                .iter()
                .map(|u| {
                    format!(
                        "{} over {}",
                        if u.unit == lw_core::bench::ErrorUnit::Character {
                            "CER"
                        } else {
                            "WER"
                        },
                        u.clips
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
    } else if m.wer.is_none() && m.by_unit.is_empty() {
        parts.push("no clip could be scored".into());
    } else {
        parts.push(format!(
            "over {} clip{}",
            m.scored_clips,
            if m.scored_clips == 1 { "" } else { "s" }
        ));
    }

    if !m.scored_languages.is_empty() {
        parts.push(format!("in {}", m.scored_languages.join("/")));
    }
    if m.skipped_clips > 0 {
        parts.push(format!(
            "({} skipped - not this model's languages)",
            m.skipped_clips
        ));
    }

    let mut line = parts.join(" ");
    // Date only: the time of day is noise, and the point is to spot a stale figure.
    if let Some(day) = m.measured_at.get(..10) {
        line.push_str(" \u{b7} ");
        line.push_str(day);
    }
    line
}

/// Accelerator ids are spelled with either separator depending on which enum produced them.
fn same_accel(a: &str, b: &str) -> bool {
    a.replace('_', "-").eq_ignore_ascii_case(&b.replace('_', "-"))
}

/// The machine's name without its specification, for a cell rather than a paragraph.
///
/// A recorded machine reads "ASUS Zenbook A16 - Snapdragon X2 Elite Extreme X2E94100, 48 GB,
/// Windows 11 build 28000 ARM64, idle". The first segment identifies it; the rest is detail the
/// tooltip carries.
fn short_machine(machine: &str) -> String {
    machine.split(" - ").next().unwrap_or(machine).trim().to_string()
}

/// `qnn_npu` reads as `npu` in a column this narrow; everything else keeps its own name.
fn short_hardware(hardware: &str) -> &str {
    match hardware {
        "qnn_npu" | "qnn-npu" => "npu",
        other => other,
    }
}

fn format_bytes(b: u64) -> String {
    format!("{:.0} MiB", b as f64 / (1024.0 * 1024.0))
}

/// How much of the row the name takes, against the columns' own portions.
///
/// The name is the widest share on purpose. In the web build it was the only thing that could
/// shrink, and "SenseVoice Small (int8, sherpa-onnx export)" rendered as the single letter "S".
const NAME_PORTION: u16 = 18;

/// The share beside it that holds the recommendation star and the role pills.
const BADGE_PORTION: u16 = 13;

/// One column of the table.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Family,
    Download,
    Languages,
    Speed,
    Accuracy,
    Mine,
    Status,
}

impl Col {
    fn label(self) -> &'static str {
        match self {
            Col::Family => "FAMILY",
            Col::Download => "DOWNLOAD",
            Col::Languages => "LANGUAGES",
            Col::Speed => "RTF (A16)",
            Col::Accuracy => "ACCURACY",
            Col::Mine => "ON YOUR MACHINE",
            Col::Status => "STATUS",
        }
    }

    fn portion(self) -> u16 {
        match self {
            Col::Family => 8,
            Col::Download => 8,
            Col::Languages => 10,
            Col::Speed => 16,
            Col::Accuracy => 15,
            Col::Mine => 12,
            Col::Status => 9,
        }
    }

    fn cell<'a>(
        self,
        entry: &'a EntryView,
        mine: &'a [lw_app::LocalMeasurement],
        _wide: bool,
    ) -> Element<'a, Message> {
        match self {
            Col::Family => widgets::sub(lw_app::catalog::family_label(&entry.engine)).into(),
            Col::Download => match entry.download_bytes {
                Some(b) => widgets::body(format_bytes(b)).into(),
                None => widgets::sub("-").into(),
            },
            Col::Languages => iced::widget::text(entry.language_summary.clone())
                .size(12)
                .color(theme::TEXT_DIM)
                .wrapping(iced::widget::text::Wrapping::None)
                .into(),
            Col::Speed => match entry.a16_reference() {
                Some(reference) => row![
                    widgets::mono(format!("{:.4}", reference.rtf)),
                    widgets::chip(short_hardware(reference.hardware.label())),
                ]
                .spacing(5)
                .into(),
                None => widgets::sub("-").into(),
            },
            Col::Accuracy => match entry.a16_reference().and_then(|m| m.wer) {
                Some(wer) => widgets::measured("WER", wer).into(),
                None => widgets::sub("-").into(),
            },
            Col::Mine => match pick_local(mine, entry.best_hardware) {
                None => widgets::sub("not yet").into(),
                Some(local) => {
                    let mut r = row![widgets::chip(short_hardware(&local.accelerator))]
                        .spacing(5)
                        .align_y(iced::Alignment::Center);
                    match headline_rate(local) {
                        Some((rate, extra)) => {
                            r = r.push(
                                iced::widget::text(format!("{:.1}%", rate * 100.0))
                                    .size(13)
                                    .font(iced::Font::MONOSPACE)
                                    .color(theme::ACCENT),
                            );
                            if !extra.is_empty() {
                                r = r.push(widgets::chip(extra));
                            }
                        }
                        // A record stored before both totals were kept has nothing to recover.
                        None if local.scored_clips > 0 => r = r.push(widgets::sub("re-run")),
                        None => r = r.push(widgets::sub("-")),
                    }
                    r = r.push(widgets::sub(format!(
                        "{:.3}",
                        local.warm_rtf.unwrap_or(local.cold_rtf)
                    )));
                    r.into()
                }
            },
            Col::Status => match entry.install_state {
                InstallState::Installed => widgets::badge_yes("Installed"),
                InstallState::Incomplete => widgets::badge("Partial", theme::ESTIMATE),
                InstallState::Unpinned => widgets::badge("n/a", theme::TEXT_DIM),
                _ => widgets::badge("Not installed", theme::TEXT_DIM),
            },
        }
    }
}

/// Which columns fit, at the same widths the stylesheet used.
///
/// Ported rather than reinvented: the web table dropped Languages below 1060 CSS pixels and Family
/// and On-your-machine below 880, because a column too narrow to read is worse than an absent one
/// -- it looks like data and is not.
fn columns_for(width: f32) -> Vec<Col> {
    if width >= 1060.0 {
        vec![
            Col::Family,
            Col::Download,
            Col::Languages,
            Col::Speed,
            Col::Accuracy,
            Col::Mine,
            Col::Status,
        ]
    } else if width >= 880.0 {
        vec![
            Col::Family,
            Col::Download,
            Col::Speed,
            Col::Accuracy,
            Col::Mine,
            Col::Status,
        ]
    } else {
        vec![Col::Download, Col::Speed, Col::Accuracy, Col::Status]
    }
}

fn header(cols: &[Col]) -> Element<'static, Message> {
    let mut r = row![
        container(iced::widget::text("MODEL").size(11).color(theme::TEXT_DIM))
            .width(Length::FillPortion(NAME_PORTION)),
        container(Space::new()).width(Length::FillPortion(BADGE_PORTION)),
    ]
    .spacing(8);
    for col in cols {
        r = r.push(
            container(iced::widget::text(col.label()).size(11).color(theme::TEXT_DIM))
                .width(Length::FillPortion(col.portion()))
                .clip(true),
        );
    }
    container(r).padding(Padding::from([6, 4])).into()
}

/// The local measurement worth putting in the row, out of however many accelerators were tried.
///
/// Show the newest run in the collapsed row so the value changes after a benchmark.
/// When a sweep records several accelerators in one second, prefer the selected target.
fn pick_local<'m>(
    mine: &'m [lw_app::LocalMeasurement],
    best: Option<&str>,
) -> Option<&'m lw_app::LocalMeasurement> {
    mine.iter().max_by(|a, b| {
        a.measured_at.cmp(&b.measured_at).then_with(|| {
            let a_selected = best.is_some_and(|target| same_accel(&a.accelerator, target));
            let b_selected = best.is_some_and(|target| same_accel(&b.accelerator, target));
            a_selected.cmp(&b_selected)
        })
    })
}

/// The one error rate a single-line cell shows, and a marker for whatever else was measured.
///
/// The blended figure is absent for every run that spanned words and characters, which is every
/// model claiming Chinese alongside a space-delimited language. The cell used to print a dash for
/// those, reading as "the benchmark produced nothing". It produced two things.
fn headline_rate(m: &lw_app::LocalMeasurement) -> Option<(f32, String)> {
    if !m.by_unit.is_empty() {
        let primary = m
            .by_unit
            .iter()
            .find(|u| u.unit == lw_core::bench::ErrorUnit::Word)
            .unwrap_or(&m.by_unit[0]);
        let rest: Vec<&str> = m
            .by_unit
            .iter()
            .filter(|u| u.unit != primary.unit)
            .map(|u| {
                if u.unit == lw_core::bench::ErrorUnit::Character {
                    "cer"
                } else {
                    "wer"
                }
            })
            .collect();
        let extra = if rest.is_empty() {
            String::new()
        } else {
            format!("+{}", rest.join("/"))
        };
        return Some((primary.rate, extra));
    }
    m.wer.map(|w| (w, String::new()))
}
