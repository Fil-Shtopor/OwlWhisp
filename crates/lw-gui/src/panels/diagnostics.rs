//! The Diagnostics tab.
//!
//! A port of `app/frontend/src/panels/Diagnostics.tsx`. Its whole job is to answer "is the NPU
//! being used, and if not, why not" without ever guessing: every value here was observed on this
//! machine, and the four that usually disagree -- the library is present, the provider registered,
//! it enumerated a device, it is usable -- are shown separately rather than collapsed into one
//! optimistic verdict.

use iced::widget::{column, container, row, Space};
use iced::Element;
use lw_app::Diagnostics;

use crate::{theme, widgets};

#[derive(Debug, Clone)]
pub enum Message {
    Refresh,
}

pub struct State {
    report: Diagnostics,
}

impl State {
    pub fn new() -> Self {
        Self {
            report: lw_app::diagnostics::collect(env!("CARGO_PKG_VERSION")),
        }
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Refresh => self.report = lw_app::diagnostics::collect(env!("CARGO_PKG_VERSION")),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let d = &self.report;

        let build = widgets::card(
            column![
                widgets::heading("Build"),
                fact("App", d.app_version.to_string()),
                fact("Core", d.core_version.to_string()),
                fact("OS", format!("{} {}", d.os, d.arch)),
                // Which binary is actually running, and when it was built.
                //
                // Not decoration. A stale copy left running in the tray silently wins the
                // single-instance claim, so a freshly built one exits without a word and the
                // user goes on testing the old behaviour and reporting it as unfixed. This is
                // the line that settles it.
                fact("Running", running_exe()),
                fact("Built", build_stamp()),
            ]
            .spacing(4),
        );

        let mut rt = column![widgets::heading("ONNX Runtime")].spacing(4);
        match (&d.runtime_dir, &d.runtime_error) {
            (Some(dir), _) => {
                rt = rt.push(fact("Loaded from", dir.clone()));
                rt = rt.push(fact_flag("QNN library present", d.qnn_dll_present));
                rt = rt.push(fact_flag("QNN registered", d.qnn_registered));
                rt = rt.push(fact("NPU devices", d.qnn_npu_count.to_string()));
                rt = rt.push(fact_flag("NPU usable", d.npu_available));
                if !d.devices.is_empty() {
                    rt = rt.push(fact("Devices", d.devices.join(", ")));
                }
            }
            (None, Some(e)) => {
                rt = rt.push(widgets::prose(format!(
                    "Not loaded: {e}. Nothing below can be said about accelerators, which is not \
                     the same as saying there are none."
                )));
            }
            (None, None) => {
                rt = rt.push(widgets::sub("No runtime and no error, which should be impossible."));
            }
        }

        let mut accel = column![
            widgets::heading("Accelerators"),
            widgets::prose(
                "Present, registered, devices and usable are kept apart because they routinely \
                 disagree: a driver package can be installed while the execution provider fails to \
                 load. Only usable means acceleration."
            ),
        ]
        .spacing(6);

        if let Some(e) = &d.accelerators_error {
            accel = accel.push(widgets::sub(e.clone()));
        }
        for a in &d.accelerators {
            let flags = row![
                if a.usable {
                    widgets::badge_yes("usable")
                } else {
                    widgets::badge_no("not usable")
                },
                widgets::chip(if a.present { "present" } else { "absent" }),
                widgets::chip(if a.registered { "registered" } else { "unregistered" }),
                widgets::chip(format!("{} device(s)", a.devices)),
            ]
            .spacing(6);
            let card: Element<'_, Message> = widgets::inset(
                    column![
                        row![
                            widgets::body(a.label),
                            widgets::sub(match a.vendor {
                                Some(v) => format!("{} - {v}", a.kind_label),
                                None => a.kind_label.to_string(),
                            }),
                        ]
                        .spacing(8),
                        flags,
                        widgets::sub(a.detail.clone()),
                        widgets::sub(match a.library {
                            Some(l) => format!("library: {l}"),
                            None => "no separate provider library".to_string(),
                        }),
                    ]
                    .spacing(4),
            )
            .into();
            accel = accel.push(card);
        }

        column![
                row![
                    iced::widget::button(widgets::body("Refresh"))
                        .padding(iced::Padding::from([6, 14]))
                        .on_press(Message::Refresh),
                    widgets::sub("Probing loads the runtime, so this takes a moment."),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
                build,
                widgets::card(rt),
                widgets::card(accel),
                Space::new(0, 8),
            ]
            .spacing(12)
            .padding(iced::Padding::from([0, 8]))
        .into()
    }
}

/// The executable this window is running from.
fn running_exe() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// When that executable was last written, which for a build directory is when it was built.
fn build_stamp() -> String {
    let Ok(path) = std::env::current_exe() else {
        return "unknown".into();
    };
    let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
        return "unknown".into();
    };
    let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return "unknown".into();
    };
    // Whole seconds, formatted by hand: a date crate for one line in one panel is a dependency to
    // audit and pin for the rest of the project's life.
    let secs = since.as_secs();
    let days = secs / 86_400;
    let (h, m, sec) = (secs % 86_400 / 3600, secs % 3600 / 60, secs % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{sec:02} UTC")
}

/// Days since the Unix epoch to a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn fact(label: &str, value: String) -> Element<'_, Message> {
    row![
        container(iced::widget::text(label).size(11).color(theme::TEXT_DIM)).width(160),
        widgets::body(value),
    ]
    .spacing(8)
    .into()
}

fn fact_flag(label: &str, yes: bool) -> Element<'_, Message> {
    row![
        container(iced::widget::text(label).size(11).color(theme::TEXT_DIM)).width(160),
        if yes {
            widgets::badge_yes("yes")
        } else {
            widgets::badge_no("no")
        },
    ]
    .spacing(8)
    .into()
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn the_epoch_and_a_few_known_days_convert_correctly() {
        // Hand-rolled rather than pulled from a date crate, so it is worth proving on the dates
        // that catch an off-by-one: the epoch, a leap day, and the century that is not a leap year
        // by the four-year rule alone.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(1), (1970, 1, 2));
        assert_eq!(civil_from_days(365), (1971, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }
}
