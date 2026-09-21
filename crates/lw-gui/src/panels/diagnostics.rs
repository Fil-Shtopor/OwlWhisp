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
