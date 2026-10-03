//! The Diagnostics tab.
//!
//! A port of `app/frontend/src/panels/Diagnostics.tsx`. Its whole job is to answer "is the NPU
//! being used, and if not, why not" without ever guessing: every value here was observed on this
//! machine, and the four that usually disagree -- the library is present, the provider registered,
//! it enumerated a device, it is usable -- are shown separately rather than collapsed into one
//! optimistic verdict.

use iced::Element;
use iced::widget::{Space, button, column, container, row};
use lw_app::Diagnostics;

use crate::{theme, widgets};

#[derive(Debug, Clone)]
pub enum Message {
    Refresh,
    /// Open the official vendor page for an optional execution provider. The page is opened, but
    /// no driver, SDK or installer is ever downloaded or run by OwlWhisp.
    OpenProviderSetup(&'static str),
}

pub struct State {
    report: Diagnostics,
    action_error: Option<String>,
}

impl State {
    pub fn new() -> Self {
        Self {
            report: lw_app::diagnostics::collect(env!("CARGO_PKG_VERSION")),
            action_error: None,
        }
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Refresh => {
                self.action_error = None;
                self.report = lw_app::diagnostics::collect(env!("CARGO_PKG_VERSION"));
            }
            Message::OpenProviderSetup(url) => {
                self.action_error = lw_platform::browser::open_https(url).err().map(|e| e.to_string());
            }
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

        // Whether the system-wide keyboard hook is being called at all.
        //
        // Every shortcut this app cannot register with the window manager runs through that hook:
        // combinations with Meta in them, and bindings made of modifiers alone. When the hook is
        // not called, those do nothing and there is no other symptom -- the settings look right,
        // the log says the hook installed, and the ordinary Ctrl+Space binding keeps working
        // because the window manager delivers that one. This counts the keys the hook has
        // actually been handed, so "zero after you have typed" says plainly which of the two it
        // is. Not for tidiness: it took an afternoon to establish by other means.
        let hook = widgets::card(
            column![
                widgets::heading("Keyboard hook"),
                match lw_platform::platform().keyboard_hook_keys_seen() {
                    Some(0) => column![
                        fact("Keys seen", "0".into()),
                        widgets::prose(
                            "Type anything and come back. If this is still zero, the hook is \
                             installed but never called, and shortcuts with Meta in them -- and \
                             bindings with no key at all -- cannot work on this machine.",
                        ),
                    ]
                    .spacing(4),
                    Some(n) => column![
                        fact("Keys seen", n.to_string()),
                        widgets::sub("The hook is live; every kind of binding can be detected."),
                    ]
                    .spacing(4),
                    None => column![widgets::sub(
                        "This operating system has no keyboard hook in this build.",
                    )],
                },
            ]
            .spacing(6),
        );

        let mut rt = column![widgets::heading("ONNX Runtime")].spacing(4);
        match (&d.runtime_dir, &d.runtime_error) {
            (Some(dir), _) => {
                rt = rt.push(fact("Loaded from", dir.clone()));
                if d.accelerators.iter().any(|a| a.id == "qnn_npu") {
                    rt = rt.push(fact_flag("QNN library present", d.qnn_dll_present));
                    rt = rt.push(fact_flag("QNN registered", d.qnn_registered));
                    rt = rt.push(fact("NPU devices", d.qnn_npu_count.to_string()));
                    rt = rt.push(fact_flag("NPU usable", d.npu_available));
                }
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
                rt = rt.push(widgets::sub(
                    "No runtime and no error, which should be impossible.",
                ));
            }
        }

        let mut accel = column![
            widgets::heading("Accelerators"),
            widgets::prose(
                "All known accelerators are listed. Unavailable means the required hardware or this app's platform support is absent; app runtime missing means compatible hardware was detected but its provider is not installed. Only usable means acceleration."
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
                } else if a.hardware_present && !a.present {
                    widgets::badge("app runtime missing", theme::ESTIMATE)
                } else {
                    widgets::badge_no("Unavailable")
                },
                widgets::chip(if a.present { "present" } else { "absent" }),
                widgets::chip(if a.registered {
                    "registered"
                } else {
                    "unregistered"
                }),
                widgets::chip(format!("{} device(s)", a.devices)),
            ]
            .spacing(6);
            let mut detail = column![
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
            .spacing(4);
            if let Some(setup) = provider_setup(a.id, a.usable, a.hardware_present) {
                detail = detail.push(
                    row![
                        button(widgets::button_label(setup.label))
                            .padding(iced::Padding::from([5, 10]))
                            .style(theme::action(false))
                            .on_press(Message::OpenProviderSetup(setup.url)),
                        widgets::sub(setup.note),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                );
            }
            let card: Element<'_, Message> = widgets::inset(detail).into();
            accel = accel.push(card);
        }

        let mut page = column![
            row![
                iced::widget::button(widgets::button_label("Refresh"))
                    .padding(iced::Padding::from([6, 14]))
                    .on_press(Message::Refresh)
                    .style(theme::action(false)),
                widgets::sub("Probing loads the runtime, so this takes a moment."),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center),
            build,
            hook,
            widgets::card(rt),
            widgets::card(accel),
            Space::new().height(8),
        ]
        .spacing(12)
        .padding(iced::Padding::from([0, 8]));
        if let Some(e) = &self.action_error {
            page = page.push(widgets::card(widgets::prose(format!(
                "Could not open the setup page: {e}"
            ))));
        }
        page.into()
    }
}

/// An explicit, official next step for providers that OwlWhisp cannot redistribute. The button
/// only appears when that particular provider is not usable; the runtime's portable WebGPU path
/// is bundled, so it has no separate installer here.
struct ProviderSetup {
    label: &'static str,
    note: &'static str,
    url: &'static str,
}

fn provider_setup(id: &str, usable: bool, hardware_present: bool) -> Option<ProviderSetup> {
    if usable || !hardware_present {
        return None;
    }
    match id {
        "cuda" => Some(ProviderSetup {
            label: "Open CUDA requirements",
            note: "Requires OwlWhisp's GPU runtime, CUDA 13 and cuDNN 9. Installing a driver alone cannot add the provider DLL.",
            url: "https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html",
        }),
        "tensor_rt" => Some(ProviderSetup {
            label: "Open TensorRT setup",
            note: "Requires a compatible NVIDIA driver, CUDA and cuDNN; NVIDIA may require sign-in.",
            url: "https://developer.nvidia.com/tensorrt/download",
        }),
        "direct_ml" => Some(ProviderSetup {
            label: "Open DirectML guide",
            note: "Requires Windows 10 version 1903 or later and a Direct3D 12-capable GPU.",
            url: "https://learn.microsoft.com/windows/ai/directml/dml",
        }),
        "open_vino" => Some(ProviderSetup {
            label: "Open OpenVINO setup",
            note: "Install the Intel OpenVINO runtime, then restart OwlWhisp.",
            url: "https://www.intel.com/content/www/us/en/developer/tools/openvino-toolkit-download.html",
        }),
        "vitis_ai" => Some(ProviderSetup {
            label: "Open Ryzen AI setup",
            note: "Requires the AMD Ryzen AI software stack for a supported XDNA NPU.",
            url: "https://www.amd.com/en/products/software/ryzen-ai.html",
        }),
        _ => None,
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
