//! The Models tab.
//!
//! A port of `app/frontend/src/panels/Models.tsx`, structure for structure: the machine card, the
//! models root with a refresh, the estimate disclaimer, the role picker, the glossary, and the
//! table with its expandable rows. Every decision it used to make in TypeScript -- which model to
//! suggest for a role and a language, which accelerator can run what, how to group by maker --
//! now comes from `lw_app`, so this file only draws.

use std::collections::BTreeSet;
use std::fmt;

use iced::widget::{button, column, container, pick_list, responsive, row, scrollable, Space};
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
    ToggleRow(String),
    ExpandAll,
    CollapseAll,
    LanguageSelected(String),
    GlossaryToggled,
    AskDelete(String),
    CancelDelete,
}

pub struct State {
    catalog: Result<CatalogView, String>,
    mine: std::collections::BTreeMap<String, Vec<lw_app::LocalMeasurement>>,
    open: BTreeSet<String>,
    language: String,
    glossary: bool,
    confirm_delete: Option<String>,
    languages: Vec<Lang>,
}

impl State {
    pub fn new() -> Self {
        let mut s = Self {
            catalog: Err("not loaded".into()),
            mine: Default::default(),
            open: Default::default(),
            language: String::new(),
            glossary: false,
            confirm_delete: None,
            languages: Vec::new(),
        };
        s.load();
        s
    }

    fn load(&mut self) {
        // The app's own directory, not `default_models_root`: that is where the Tauri build
        // installed everything, and looking anywhere else reports an empty catalog.
        self.catalog = lw_app::catalog::build(&lw_app::paths::models_root());
        let path = lw_app::measurements::path_for(&lw_app::paths::settings_path());
        self.mine = lw_app::LocalMeasurements::load(&path).models;
        self.languages = self.build_languages();
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
                    name: e
                        .language_names
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| code.clone()),
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

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Refresh => self.load(),
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
        }
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

        scrollable(
            column![
                machine_card(view),
                row![
                    button(widgets::body("Refresh"))
                        .padding(Padding::from([6, 14]))
                        .on_press(Message::Refresh),
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
            .padding(Padding::from([0, 8])),
        )
        .height(Length::Fill)
        .into()
    }

    fn role_picks<'a>(&'a self, view: &'a CatalogView) -> Element<'a, Message> {
        let measured = lw_app::catalog::measured_languages(&view.entries);
        let selected = self
            .languages
            .iter()
            .find(|l| l.code == self.language)
            .cloned();

        let head = row![
            column![
                widgets::heading("Which one should you use?"),
                widgets::sub("A suggestion, not a switch - nothing here changes which model runs."),
            ]
            .spacing(2),
            Space::new(Length::Fill, 0),
            widgets::sub("Language"),
            pick_list(self.languages.clone(), selected, |l: Lang| {
                Message::LanguageSelected(l.code)
            })
            .text_size(14),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        let mut body = column![head].spacing(10);

        for role in ModelRole::ALL {
            let mut block = column![row![
                widgets::badge(role.label(), theme::TEXT_DIM),
                widgets::sub(role.blurb()),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)]
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

    fn glossary_block<'a>(&'a self, view: &'a CatalogView) -> Element<'a, Message> {
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
                .push(widgets::prose(format!(
                    "The error rate in the Accuracy column is neither the model publisher's \
                     published figure nor something measured live on this computer. It was \
                     measured by this project with `lw bench` on {}, and committed to the catalog.",
                    view.machine
                )))
                .push(widgets::prose(
                    "Speed is an estimate - arithmetic on the model's speed tier and the detected \
                     hardware, marked with a tilde, never a measurement. A green error rate is a \
                     measurement. The quality pill beside it is an editorial ranking of the model \
                     family, not either of those.",
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
            button(widgets::body(if all_open {
                "Collapse all"
            } else {
                "Expand all"
            }))
            .padding(Padding::from([6, 14]))
            .on_press(if all_open {
                Message::CollapseAll
            } else {
                Message::ExpandAll
            }),
            Space::new(Length::Fill, 0),
            widgets::sub(format!("{} models", view.entries.len())),
        ]
        .align_y(iced::Alignment::Center);

        // `responsive` reports the width actually available, which is what the breakpoints need.
        let table = responsive(move |size| self.table_body(view, size.width));

        widgets::card(column![toolbar, table].spacing(8)).into()
    }

    fn table_body<'a>(&'a self, view: &'a CatalogView, width: f32) -> Element<'a, Message> {
        let cols = columns_for(width);
        let mut body = column![header(&cols)].spacing(0);
        body = body.push(iced::widget::horizontal_rule(1).style(theme::rule));

        for (vendor, rows) in
            lw_app::catalog::group_by_vendor(&view.entries, view.recommended.as_deref())
        {
            body = body.push(Space::new(0, 8));
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
                body = body.push(iced::widget::horizontal_rule(1).style(theme::rule));
            }
        }

        body = body.push(Space::new(0, 8));
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
        let mine: &[lw_app::LocalMeasurement] = self
            .mine
            .get(&entry.id)
            .map(|v| v.as_slice())
            .unwrap_or(EMPTY);

        let mut name_cell = row![
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
        if recommended {
            name_cell = name_cell.push(widgets::badge("* Recommended", theme::ACCENT));
        }
        for r in &entry.roles {
            name_cell = name_cell.push(widgets::badge(r.label, theme::TEXT_DIM));
        }

        let mut line = row![container(name_cell)
            .width(Length::FillPortion(NAME_PORTION))
            .clip(true)]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        for col in cols {
            line = line.push(
                container(col.cell(entry, mine))
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

        body = body.push(Space::new(0, 6));
        body = body.push(accelerator_table(entry, mine));

        if let Some(dir) = &entry.install_dir {
            body = body.push(widgets::sub(dir.clone()));
        }
        if let Some(url) = &entry.upstream_url {
            body = body.push(widgets::sub(format!("Upstream: {url}")));
        }

        let confirming = self.confirm_delete.as_deref() == Some(entry.id.as_str());
        let buttons: Element<'_, Message> = if confirming {
            // Deleting is irreversible, so the confirmation sits inline where the row is rather
            // than over the whole window, and backing out is the easier of the two.
            row![
                widgets::sub("Delete this model's files?"),
                button(widgets::body("Cancel"))
                    .padding(Padding::from([6, 12]))
                    .on_press(Message::CancelDelete),
                button(iced::widget::text("Delete").size(14).color(theme::BAD))
                    .padding(Padding::from([6, 12])),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            row![
                button(iced::widget::text("Delete").size(14).color(theme::BAD))
                    .padding(Padding::from([6, 12]))
                    .on_press(Message::AskDelete(entry.id.clone()))
            ]
            .into()
        };
        body = body.push(buttons);

        widgets::inset(body).into()
    }
}

fn machine_card(view: &CatalogView) -> Element<'_, Message> {
    widgets::card(
        column![
            iced::widget::text("THIS MACHINE")
                .size(11)
                .color(theme::TEXT_DIM),
            widgets::mono(view.machine.clone()),
            widgets::sub("Recommendations and estimates below are computed for this machine."),
        ]
        .spacing(4),
    )
    .into()
}

fn disclaimer(text: &str) -> Element<'_, Message> {
    // A left rule in the accent colour, as the stylesheet has it: this is the one piece of prose
    // in the tab that must not be skimmed past.
    container(
        row![
            container(Space::new(3, Length::Fill)).style(|_t| container::Style {
                background: Some(theme::ACCENT.into()),
                ..Default::default()
            }),
            widgets::prose(text),
        ]
        .spacing(12),
    )
    .padding(12)
    .style(theme::inset)
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

fn accelerator_table<'a>(
    entry: &'a EntryView,
    mine: &'a [lw_app::LocalMeasurement],
) -> Element<'a, Message> {
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

    let mut table = column![row![
        container(
            iced::widget::text("Accelerator")
                .size(11)
                .color(theme::TEXT_DIM)
        )
        .width(Length::FillPortion(3)),
        container(
            iced::widget::text("Runs this model")
                .size(11)
                .color(theme::TEXT_DIM)
        )
        .width(Length::FillPortion(3)),
        container(
            iced::widget::text("Measured on this machine")
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
    .spacing(8)]
    .spacing(4);

    for a in entry
        .accelerators
        .iter()
        .filter(|a| a.supported || has_number(a))
    {
        let runs: Element<'_, Message> = if a.supported {
            widgets::badge_yes("yes")
        } else {
            widgets::badge_no("no")
        };
        // Measured here and in the catalog stay in separate columns: they were taken on different
        // machines on different days, and one column would invite reading one as a check on the
        // other.
        let here: Element<'_, Message> =
            match mine.iter().find(|m| same_accel(&m.accelerator, a.id)) {
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
                r.into()
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
    table.into()
}

/// Accelerator ids are spelled with either separator depending on which enum produced them.
fn same_accel(a: &str, b: &str) -> bool {
    a.replace('_', "-").eq_ignore_ascii_case(&b.replace('_', "-"))
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
const NAME_PORTION: u16 = 26;

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
            Col::Speed => "SPEED",
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
            Col::Speed => 12,
            Col::Accuracy => 15,
            Col::Mine => 12,
            Col::Status => 9,
        }
    }

    fn cell<'a>(
        self,
        entry: &'a EntryView,
        mine: &'a [lw_app::LocalMeasurement],
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
            Col::Speed => match entry.estimated_rtf {
                Some(rtf) => {
                    let mut r = row![widgets::estimated(rtf)].spacing(5);
                    if let Some(hw) = entry.best_hardware {
                        r = r.push(widgets::chip(short_hardware(hw)));
                    }
                    r.align_y(iced::Alignment::Center).into()
                }
                None => Space::new(0, 0).into(),
            },
            Col::Accuracy => {
                let mut r = row![widgets::badge(entry.quality_label, theme::TEXT_DIM)].spacing(5);
                let point = entry
                    .measured_reference
                    .as_ref()
                    .or(entry.measurements.first());
                if let Some((m, wer)) = point.and_then(|m| m.wer.map(|w| (m, w))) {
                    r = r.push(widgets::measured("WER", wer));
                    r = r.push(widgets::chip(short_hardware(m.hardware.label())));
                }
                r.align_y(iced::Alignment::Center).into()
            }
            Col::Mine => match pick_local(mine, entry.best_hardware) {
                None => widgets::sub("not yet").into(),
                Some(local) => {
                    let mut r = row![].spacing(5).align_y(iced::Alignment::Center);
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
    let mut r = row![container(
        iced::widget::text("MODEL").size(11).color(theme::TEXT_DIM)
    )
    .width(Length::FillPortion(NAME_PORTION))]
    .spacing(8);
    for col in cols {
        r = r.push(
            container(
                iced::widget::text(col.label())
                    .size(11)
                    .color(theme::TEXT_DIM),
            )
            .width(Length::FillPortion(col.portion()))
            .clip(true),
        );
    }
    container(r).padding(Padding::from([6, 4])).into()
}

/// The local measurement worth putting in the row, out of however many accelerators were tried.
///
/// Prefers the accelerator this machine would actually use, because that is the run that predicts
/// what the user will experience; falls back to the fastest so a row is never blank when something
/// was measured.
fn pick_local<'m>(
    mine: &'m [lw_app::LocalMeasurement],
    best: Option<&str>,
) -> Option<&'m lw_app::LocalMeasurement> {
    if let Some(hit) = best.and_then(|b| mine.iter().find(|m| same_accel(&m.accelerator, b))) {
        return Some(hit);
    }
    mine.iter().min_by(|a, b| {
        let av = a.warm_rtf.unwrap_or(a.cold_rtf);
        let bv = b.warm_rtf.unwrap_or(b.cold_rtf);
        av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal)
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
