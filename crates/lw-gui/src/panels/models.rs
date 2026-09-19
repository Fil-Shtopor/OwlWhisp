//! The Models tab.
//!
//! A port of `app/frontend/src/panels/Models.tsx`, structure for structure: the machine card, the
//! models root with a refresh, the estimate disclaimer, the role picker, the glossary, and the
//! table with its expandable rows. The decisions it used to make in TypeScript -- which model to
//! suggest for a role and a language, which accelerator can run what -- now come from `lw_app`,
//! so this file only draws.

use egui::{Align, Layout, RichText};
use lw_app::catalog::{CatalogView, EntryView};
use lw_core::model::ModelRole;

use crate::{theme, widgets};

/// What the tab remembers between frames.
#[derive(Default)]
pub struct State {
    /// The catalog, loaded once and on demand. `Err` is kept and shown rather than retried in a
    /// loop: a broken catalog is a fact the user needs, not a reason to spin.
    catalog: Option<Result<CatalogView, String>>,
    /// Which rows are expanded.
    open: std::collections::BTreeSet<String>,
    /// The language filter for the role picker; empty means "any".
    language: String,
    /// Which model a delete has been requested for, awaiting confirmation.
    confirm_delete: Option<String>,
    /// What this machine has measured, by model id. Empty until a benchmark has been run.
    mine: std::collections::BTreeMap<String, Vec<lw_app::LocalMeasurement>>,
}

impl State {
    fn load(&mut self) {
        // The app's own directory, not `default_models_root`: that is where the Tauri build
        // installed everything, and looking anywhere else reports an empty catalog.
        let root = lw_app::paths::models_root();
        self.catalog = Some(lw_app::catalog::build(&root));
        let path = lw_app::measurements::path_for(&lw_app::paths::settings_path());
        self.mine = lw_app::LocalMeasurements::load(&path).models;
    }
}

pub fn show(ui: &mut egui::Ui, state: &mut State) {
    if state.catalog.is_none() {
        state.load();
    }
    let reload = match state.catalog.as_ref() {
        Some(Ok(view)) => {
            let view = view.clone();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| body(ui, state, &view))
                .inner
        }
        Some(Err(e)) => {
            let e = e.clone();
            widgets::card(ui, |ui| {
                widgets::heading(ui, "The catalog could not be read");
                widgets::prose(ui, e);
            });
            false
        }
        None => false,
    };
    if reload {
        state.load();
    }
}

/// Returns true when the caller should reload the catalog.
fn body(ui: &mut egui::Ui, state: &mut State, view: &CatalogView) -> bool {
    let mut reload = false;

    machine_card(ui, view);
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        if ui.button("Refresh").clicked() {
            reload = true;
        }
        ui.label(
            RichText::new(&view.models_root)
                .monospace()
                .color(theme::TEXT_DIM),
        );
    });
    ui.add_space(12.0);

    disclaimer(ui, view.estimate_disclaimer);
    ui.add_space(12.0);

    role_picks(ui, state, view);
    ui.add_space(12.0);

    glossary(ui, view);
    ui.add_space(12.0);

    table(ui, state, view);
    reload
}

fn machine_card(ui: &mut egui::Ui, view: &CatalogView) {
    widgets::card(ui, |ui| {
        ui.label(
            RichText::new("THIS MACHINE")
                .size(11.0)
                .color(theme::TEXT_DIM),
        );
        ui.label(RichText::new(&view.machine).monospace().color(theme::TEXT));
        widgets::sub(
            ui,
            "Recommendations and estimates below are computed for this machine.",
        );
    });
}

fn disclaimer(ui: &mut egui::Ui, text: &str) {
    egui::Frame::none()
        .fill(theme::BG_INSET)
        .inner_margin(egui::Margin::same(12.0))
        .rounding(egui::Rounding::same(8.0))
        .show(ui, |ui| {
            // A left rule in the accent colour, as the stylesheet has it: this is the one piece of
            // prose in the tab that must not be skimmed past.
            let r = ui.max_rect();
            ui.painter().rect_filled(
                egui::Rect::from_min_size(r.min - egui::vec2(12.0, 0.0), egui::vec2(3.0, r.height())),
                egui::Rounding::same(2.0),
                theme::ACCENT,
            );
            widgets::prose(ui, text);
        });
}

fn role_picks(ui: &mut egui::Ui, state: &mut State, view: &CatalogView) {
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                widgets::heading(ui, "Which one should you use?");
                widgets::sub(
                    ui,
                    "A suggestion, not a switch — nothing here changes which model runs.",
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                let mut names: Vec<(String, String)> = Vec::new();
                for e in &view.entries {
                    for (i, code) in e.languages.iter().enumerate() {
                        let name = e.language_names.get(i).cloned().unwrap_or_else(|| code.clone());
                        if !names.iter().any(|(c, _)| c == code) {
                            names.push((code.clone(), name));
                        }
                    }
                }
                // Sorted by name, not by code: the list is read alphabetically by a human looking
                // for theirs, and by code "Ukrainian" sits between "tt" and "ur".
                names.sort_by(|a, b| a.1.cmp(&b.1));
                let current = names
                    .iter()
                    .find(|(c, _)| *c == state.language)
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| "Any".to_string());
                egui::ComboBox::from_id_salt("pick-language")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut state.language, String::new(), "Any");
                        for (code, name) in &names {
                            ui.selectable_value(&mut state.language, code.clone(), name);
                        }
                    });
                widgets::sub(ui, "Language");
            });
        });
        ui.add_space(8.0);

        let measured = lw_app::catalog::measured_languages(&view.entries);
        let lang_name = |code: &str| -> String {
            view.entries
                .iter()
                .find_map(|e| {
                    e.languages
                        .iter()
                        .position(|l| l == code)
                        .and_then(|i| e.language_names.get(i).cloned())
                })
                .unwrap_or_else(|| code.to_string())
        };

        for role in ModelRole::ALL {
            ui.horizontal(|ui| {
                widgets::badge(ui, role.label(), theme::TEXT_DIM);
                widgets::sub(ui, role.blurb());
            });
            match lw_app::catalog::pick_for_role(&view.entries, *role, &state.language) {
                None => {
                    widgets::sub(
                        ui,
                        if state.language.is_empty() {
                            "Nothing in this catalog fills that role.".to_string()
                        } else {
                            format!(
                                "Nothing here fills that role for {}.",
                                lang_name(&state.language)
                            )
                        },
                    );
                }
                Some(pick) => {
                    ui.label(RichText::new(&pick.name).color(theme::ACCENT).strong());
                    match pick.measured_for_language(&state.language) {
                        Some(rate) => {
                            widgets::sub(
                                ui,
                                format!(
                                    "{} {:.1}% measured on {} {} clip{}",
                                    if rate.unit == lw_core::bench::ErrorUnit::Character {
                                        "CER"
                                    } else {
                                        "WER"
                                    },
                                    rate.rate * 100.0,
                                    rate.clips,
                                    lang_name(&state.language),
                                    if rate.clips == 1 { "" } else { "s" },
                                ),
                            );
                        }
                        None if !state.language.is_empty()
                            && !measured.contains(&state.language) =>
                        {
                            // The honest version of a missing number: without this the pick looks
                            // equally well-founded whether it rests on a measurement of the chosen
                            // language or on a figure from four entirely different ones.
                            widgets::sub(
                                ui,
                                format!(
                                    "Not measured: there are no {} fixtures. Ranked on this \
                                     model's overall figure, which comes from other languages.",
                                    lang_name(&state.language)
                                ),
                            );
                        }
                        None => {}
                    }
                }
            }
            ui.add_space(6.0);
        }

        widgets::prose(
            ui,
            "The roles themselves are editorial, but the pick within each one is not: it uses that \
             role's own criterion — fewest errors, least delay, smallest download, most languages. \
             For accuracy with a language chosen it uses the rate measured on that language, \
             because every candidate was scored on the same clips. On Any there is no such \
             comparison to make: a blended rate is not one measurement, so those are compared \
             coarsely and the tie goes to the model covering more languages.",
        );
    });
}

fn glossary(ui: &mut egui::Ui, view: &CatalogView) {
    egui::CollapsingHeader::new(
        RichText::new("What the numbers mean, and where they come from").strong(),
    )
    .id_salt("glossary")
    .show(ui, |ui| {
        widgets::prose(
            ui,
            format!(
                "The WER in the Accuracy column is neither the model publisher's published figure \
                 nor something measured live on this computer. It was measured by this project \
                 with `lw bench` on {}, and committed to the catalog.",
                view.machine
            ),
        );
        widgets::prose(
            ui,
            "Speed is an estimate — arithmetic on the model's speed tier and the detected \
             hardware, marked with ~, never a measurement. A green error rate is a measurement. \
             The quality pill beside it is an editorial ranking of the model family, not either \
             of those.",
        );
        widgets::prose(
            ui,
            "WER is the word error rate and CER the character error rate: the share of words, or \
             of characters for languages written without spaces, that came out wrong. Lower is \
             better in both. RTF is the real-time factor, seconds of computing per second of \
             audio. Lower is faster.",
        );
    });
}

/// One column of the table.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
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

    fn note(self) -> &'static str {
        match self {
            Col::Speed => "estimated RTF, lower is faster",
            Col::Accuracy => "tier, then the catalog WER. Lower is better.",
            Col::Mine => "error rate and RTF from your own benchmark run",
            Col::Status => "on disk",
            _ => "",
        }
    }

    /// Share of the table's width. The name column takes whatever is left.
    fn share(self) -> f32 {
        match self {
            Col::Family => 0.08,
            Col::Download => 0.08,
            Col::Languages => 0.11,
            Col::Speed => 0.12,
            Col::Accuracy => 0.17,
            Col::Mine => 0.11,
            Col::Status => 0.09,
        }
    }
}

/// Which columns fit, at the same widths the stylesheet used.
///
/// Ported rather than reinvented: the web table dropped Languages below 1060 CSS pixels and
/// Family and On-your-machine below 880, because a column that is too narrow to read is worse
/// than an absent one -- it looks like data and is not. The same three thresholds, in points.
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

/// The least of the name cell the name itself may be given, as a fraction of that cell.
///
/// Both halves of this have gone wrong once. First the badges were unshrinkable and the name was
/// the only thing that could give, so "SenseVoice Small (int8, sherpa-onnx export)" rendered as
/// "S". Then the badges were placed first and the name disappeared entirely behind them. Neither
/// side gets to take all of it.
const NAME_SHARE: f32 = 0.55;

/// Lay out one cell of a fixed width, clipped to it, and draw into it.
///
/// The clip is not cosmetic. Without it a cell whose content is a little too wide pushes every
/// cell after it along, and the last column -- Status, the one saying whether the model is even
/// installed -- slides off the right edge of the window.
fn cell<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let out = ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            let clip = ui.clip_rect().intersect(egui::Rect::everything_right_of(
                ui.max_rect().left(),
            ));
            ui.set_clip_rect(clip.intersect(egui::Rect::everything_left_of(
                ui.max_rect().right(),
            )));
            add(ui)
        },
    );
    out.inner
}

fn table(ui: &mut egui::Ui, state: &mut State, view: &CatalogView) {
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            let all_open = view.entries.iter().all(|e| state.open.contains(&e.id));
            if ui
                .button(if all_open { "Collapse all" } else { "Expand all" })
                .clicked()
            {
                if all_open {
                    state.open.clear();
                } else {
                    state.open = view.entries.iter().map(|e| e.id.clone()).collect();
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                widgets::sub(ui, format!("{} models", view.entries.len()));
            });
        });
        ui.add_space(8.0);

        let total = ui.available_width();
        let cols = columns_for(total);
        // Every cell is laid out horizontally, so the gaps between them are part of the budget.
        // Forgetting them pushed the last column off the right edge.
        let spacing = ui.spacing().item_spacing.x;
        let gaps = spacing * (cols.len() as f32 + 2.0);
        let fixed: f32 = cols.iter().map(|c| c.share() * total).sum();
        let arrow = 16.0;
        let name_w = (total - fixed - arrow - gaps).max(140.0);

        ui.horizontal(|ui| {
            cell(ui, arrow, |_| {});
            cell(ui, name_w, |ui| {
                ui.label(RichText::new("MODEL").size(11.0).color(theme::TEXT_DIM));
            });
            for col in &cols {
                cell(ui, col.share() * total, |ui| {
                    let r = ui.label(RichText::new(col.label()).size(11.0).color(theme::TEXT_DIM));
                    if !col.note().is_empty() {
                        r.on_hover_text(col.note());
                    }
                });
            }
        });
        ui.separator();

        let groups = lw_app::catalog::group_by_vendor(&view.entries, view.recommended.as_deref());
        for (vendor, rows) in groups {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(vendor.to_uppercase())
                        .size(11.0)
                        .strong()
                        .color(theme::TEXT),
                );
                widgets::sub(
                    ui,
                    format!(
                        "{} model{}",
                        rows.len(),
                        if rows.len() == 1 { "" } else { "s" }
                    ),
                );
            });
            for entry in rows {
                row(ui, state, view, entry, &cols, arrow, name_w, total);
            }
        }

        ui.add_space(8.0);
        widgets::prose(
            ui,
            "Models are grouped by who made them: the maker of the recommended model first, then \
             the makers offering the most, with anything uncredited under Other. Speed is an \
             estimate computed from the model's speed tier and this machine, never a measurement, \
             and the tilde marks it. Under Accuracy the tier is an editorial ranking of the model \
             family, while a green error rate is a real measurement and the chip beside it says \
             which accelerator it was taken on. Open a row for the full detail and the buttons.",
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut egui::Ui,
    state: &mut State,
    view: &CatalogView,
    entry: &EntryView,
    cols: &[Col],
    arrow: f32,
    name_w: f32,
    total: f32,
) {
    let open = state.open.contains(&entry.id);
    let recommended = view.recommended.as_deref() == Some(entry.id.as_str());
    let mine: Vec<lw_app::LocalMeasurement> =
        state.mine.get(&entry.id).cloned().unwrap_or_default();
    let mut toggle = false;

    ui.horizontal(|ui| {
        cell(ui, arrow, |ui| {
            // Drawn rather than typed: the default font has no glyph for the triangles the web
            // build used, and a missing glyph renders as a hollow box.
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
            let c = rect.center();
            let pts = if open {
                vec![
                    c + egui::vec2(-4.0, -2.0),
                    c + egui::vec2(4.0, -2.0),
                    c + egui::vec2(0.0, 3.0),
                ]
            } else {
                vec![
                    c + egui::vec2(-2.0, -4.0),
                    c + egui::vec2(-2.0, 4.0),
                    c + egui::vec2(3.0, 0.0),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                pts,
                theme::TEXT_DIM,
                egui::Stroke::NONE,
            ));
            if resp.clicked() {
                toggle = true;
            }
        });

        let text_w = name_w * NAME_SHARE;
        cell(ui, text_w, |ui| {
            // Truncated, never wrapped: a wrapped name makes the row three lines tall and the
            // table stops being scannable, which is the only thing a table is for.
            let r = ui.add(
                egui::Label::new(RichText::new(&entry.name).color(theme::TEXT))
                    .truncate()
                    .sense(egui::Sense::click()),
            );
            if r.clicked() {
                toggle = true;
            }
            r.on_hover_text(&entry.name);
        });
        cell(ui, name_w - text_w - ui.spacing().item_spacing.x, |ui| {
            if recommended {
                widgets::badge(ui, "* Recommended", theme::ACCENT);
            }
            for r in &entry.roles {
                widgets::badge(ui, r.label, theme::TEXT_DIM);
            }
        });

        for col in cols {
            cell(ui, col.share() * total, |ui| {
                draw_col(ui, *col, entry, &mine);
            });
        }
    });

    if toggle {
        if open {
            state.open.remove(&entry.id);
        } else {
            state.open.insert(entry.id.clone());
        }
    }
    if open {
        expansion(ui, state, entry, &mine);
    }
    ui.separator();
}

fn draw_col(ui: &mut egui::Ui, col: Col, entry: &EntryView, mine: &[lw_app::LocalMeasurement]) {
    match col {
        Col::Family => {
            widgets::sub(ui, lw_app::catalog::family_label(&entry.engine));
        }
        Col::Download => match entry.download_bytes {
            Some(b) => {
                ui.label(RichText::new(format_bytes(b)).color(theme::TEXT));
            }
            None => {
                widgets::sub(ui, "-");
            }
        },
        Col::Languages => {
            ui.add(
                egui::Label::new(
                    RichText::new(&entry.language_summary)
                        .size(12.0)
                        .color(theme::TEXT_DIM),
                )
                .truncate()
                .sense(egui::Sense::hover()),
            )
            .on_hover_text(entry.language_names.join(", "));
        }
        Col::Speed => {
            if let Some(rtf) = entry.estimated_rtf {
                widgets::estimate(ui, rtf);
                if let Some(hw) = entry.best_hardware {
                    widgets::chip(ui, short_hardware(hw));
                }
            }
        }
        Col::Accuracy => {
            widgets::badge(ui, entry.quality_label, theme::TEXT_DIM);
            let point = entry
                .measured_reference
                .as_ref()
                .or(entry.measurements.first());
            if let Some((m, wer)) = point.and_then(|m| m.wer.map(|w| (m, w))) {
                widgets::measured_rate(ui, "WER", wer);
                widgets::chip(ui, short_hardware(m.hardware.label()));
            }
        }
        Col::Mine => match pick_local(mine, entry.best_hardware) {
            None => {
                widgets::sub(ui, "not yet");
            }
            Some(local) => {
                match headline_rate(local) {
                    Some((rate, extra)) => {
                        ui.label(
                            RichText::new(format!("{:.1}%", rate * 100.0))
                                .monospace()
                                .color(theme::ACCENT),
                        );
                        if !extra.is_empty() {
                            widgets::chip(ui, &extra);
                        }
                    }
                    // A record stored before both totals were kept has nothing to recover.
                    None if local.scored_clips > 0 => {
                        widgets::sub(ui, "re-run");
                    }
                    None => {
                        widgets::sub(ui, "-");
                    }
                }
                widgets::sub(
                    ui,
                    format!("{:.3}", local.warm_rtf.unwrap_or(local.cold_rtf)),
                );
            }
        },
        Col::Status => match entry.install_state {
            lw_core::model::InstallState::Installed => {
                widgets::badge_yes(ui, "Installed");
            }
            lw_core::model::InstallState::Incomplete => {
                widgets::badge(ui, "Partial", theme::ESTIMATE);
            }
            lw_core::model::InstallState::Unpinned => {
                widgets::badge(ui, "n/a", theme::TEXT_DIM);
            }
            _ => {
                widgets::badge(ui, "Not installed", theme::TEXT_DIM);
            }
        },
    }
}

/// `qnn_npu` reads as `npu` in a column this narrow; everything else keeps its own name.
fn short_hardware(hardware: &str) -> &str {
    match hardware {
        "qnn_npu" | "qnn-npu" => "npu",
        other => other,
    }
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
    if let Some(hit) =
        best.and_then(|best| mine.iter().find(|m| same_accel(&m.accelerator, best)))
    {
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

fn expansion(
    ui: &mut egui::Ui,
    state: &mut State,
    entry: &EntryView,
    mine: &[lw_app::LocalMeasurement],
) {
    egui::Frame::none()
        .fill(theme::BG_INSET)
        .inner_margin(egui::Margin::same(12.0))
        .rounding(egui::Rounding::same(8.0))
        .show(ui, |ui| {
            widgets::prose(ui, entry.description.clone());
            ui.add_space(6.0);

            fact(ui, "Languages", &entry.language_names.join(", "));
            fact(ui, "Licence", &entry.licence);
            if let Some(b) = entry.download_bytes {
                fact(ui, "Download", &format_bytes(b));
            }
            if let Some(b) = entry.disk_bytes {
                fact(ui, "On disk", &format_bytes(b));
            }
            fact(ui, "Quality", entry.quality_label);
            fact(ui, "Speed", entry.speed_label);

            ui.add_space(8.0);
            accelerator_table(ui, entry, mine);

            ui.add_space(8.0);
            if let Some(dir) = &entry.install_dir {
                ui.label(RichText::new(dir).monospace().size(11.0).color(theme::TEXT_DIM));
            }
            if let Some(url) = &entry.upstream_url {
                ui.hyperlink_to(RichText::new(url).size(11.0), url);
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let confirming = state.confirm_delete.as_deref() == Some(entry.id.as_str());
                if !confirming {
                    if ui
                        .button(RichText::new("Delete").color(theme::BAD))
                        .clicked()
                    {
                        state.confirm_delete = Some(entry.id.clone());
                    }
                } else {
                    // Deleting is irreversible, so the confirmation sits inline where the row is
                    // rather than over the whole window, and the default is to back out.
                    widgets::sub(ui, "Delete this model's files?");
                    if ui.button("Cancel").clicked() {
                        state.confirm_delete = None;
                    }
                    if ui
                        .button(RichText::new("Delete").color(theme::BAD))
                        .clicked()
                    {
                        state.confirm_delete = None;
                    }
                }
            });
        });
}

fn accelerator_table(ui: &mut egui::Ui, entry: &EntryView, mine: &[lw_app::LocalMeasurement]) {
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
    let rows: Vec<_> = entry
        .accelerators
        .iter()
        .filter(|a| a.supported || has_number(a))
        .collect();

    egui::Grid::new(format!("accel-{}", entry.id))
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            for label in [
                "Accelerator",
                "Runs this model",
                "Measured on this machine",
                "In the catalog",
            ] {
                ui.label(RichText::new(label).size(11.0).color(theme::TEXT_DIM));
            }
            ui.end_row();
            for a in rows {
                ui.label(a.label);
                if a.supported {
                    widgets::badge_yes(ui, "✓ yes");
                } else {
                    widgets::badge_no(ui, "✕ no");
                }
                // Measured here and in the catalog stay in separate columns: they were taken on
                // different machines on different days, and one column would invite reading one
                // as a check on the other.
                match mine.iter().find(|m| same_accel(&m.accelerator, a.id)) {
                    Some(local) => {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "RTF {:.4}",
                                    local.warm_rtf.unwrap_or(local.cold_rtf)
                                ))
                                .monospace()
                                .color(theme::TEXT),
                            );
                            for u in &local.by_unit {
                                widgets::measured_rate(
                                    ui,
                                    if u.unit == lw_core::bench::ErrorUnit::Character {
                                        "CER"
                                    } else {
                                        "WER"
                                    },
                                    u.rate,
                                );
                            }
                        });
                    }
                    None => {
                        widgets::sub(ui, if a.supported { "not measured yet" } else { "-" });
                    }
                }
                match entry
                    .measurements
                    .iter()
                    .find(|m| same_accel(m.hardware.label(), a.id))
                {
                    Some(m) => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("RTF {:.4}", m.rtf))
                                    .monospace()
                                    .color(theme::TEXT),
                            );
                            if let Some(w) = m.wer {
                                widgets::measured_rate(ui, "· WER", w);
                            }
                        });
                    }
                    None => {
                        widgets::sub(ui, "—");
                    }
                }
                ui.end_row();
            }
        });

    let mut by_reason: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for a in entry.accelerators.iter().filter(|a| !a.supported && !has_number(a)) {
        by_reason
            .entry(a.reason.as_deref().unwrap_or("not supported"))
            .or_default()
            .push(a.label);
    }
    for (reason, labels) in by_reason {
        ui.horizontal_wrapped(|ui| {
            widgets::badge_no(ui, "✕ cannot run");
            widgets::sub(ui, format!("{} — {reason}.", labels.join(", ")));
        });
    }
}

/// Accelerator ids are spelled with either separator depending on which enum produced them.
fn same_accel(a: &str, b: &str) -> bool {
    a.replace('_', "-").eq_ignore_ascii_case(&b.replace('_', "-"))
}

fn fact(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(11.0).color(theme::TEXT_DIM));
        ui.label(RichText::new(value).color(theme::TEXT));
    });
}

fn format_bytes(b: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    format!("{:.0} MiB", b as f64 / MIB)
}
