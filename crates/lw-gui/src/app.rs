//! The window: a tab bar and whichever panel is selected.
//!
//! The tab order is the one the web front end settled on -- Dictate, Settings, Models, Benchmark,
//! Diagnostics -- because it is the order of how often a tab is wanted, and moving it would be a
//! change nobody asked for in a commit that is supposed to change nothing a user can see.

use egui::{Align, Layout, RichText};

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

/// Everything the window holds between frames.
pub struct App {
    pub tab: Tab,
    pub models: panels::models::State,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        Self {
            tab: Tab::Dictate,
            models: panels::models::State::default(),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("tabs")
            .frame(
                egui::Frame::none()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin {
                        left: 12.0,
                        right: 12.0,
                        top: 8.0,
                        bottom: 0.0,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for tab in Tab::ALL {
                        let selected = self.tab == tab;
                        let text = RichText::new(tab.label())
                            .size(15.0)
                            .color(if selected { theme::TEXT } else { theme::TEXT_DIM });
                        if ui.selectable_label(selected, text).clicked() {
                            self.tab = tab;
                        }
                    }
                });
                ui.add_space(6.0);
                let y = ui.max_rect().bottom();
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    y,
                    egui::Stroke::new(1.0_f32, theme::BORDER),
                );
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::same(16.0)),
            )
            .show(ctx, |ui| {
                ui.with_layout(Layout::top_down(Align::Min), |ui| match self.tab {
                    Tab::Dictate => panels::dictate::show(ui),
                    Tab::Settings => panels::settings::show(ui),
                    Tab::Models => panels::models::show(ui, &mut self.models),
                    Tab::Benchmark => panels::benchmark::show(ui),
                    Tab::Diagnostics => panels::diagnostics::show(ui),
                });
            });
    }
}
