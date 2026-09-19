//! The handful of shapes the web front end used everywhere, redrawn.
//!
//! Pills, chips and dimmed sub-lines are not decoration here: the project's rule is that an
//! estimate and a measurement must never look alike, and the shapes are how that rule is kept
//! visible. They are gathered in one place so the rule is enforced once rather than remembered at
//! every call site.

use egui::{Align, Color32, FontId, Layout, Response, RichText, Rounding, Sense, Ui, Vec2};

use crate::theme;

/// A rounded pill with a tinted background, matching `.badge` in the stylesheet.
pub fn badge(ui: &mut Ui, text: &str, colour: Color32) -> Response {
    let font = FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, colour);
    let pad = Vec2::new(10.0, 2.0);
    let (rect, response) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(
            rect,
            Rounding::same(999.0),
            colour.linear_multiply(0.15),
        );
        ui.painter().galley(rect.min + pad, galley, colour);
    }
    response
}

/// A badge in the palette's "yes" green.
pub fn badge_yes(ui: &mut Ui, text: &str) -> Response {
    badge(ui, text, theme::GOOD)
}

/// A badge in the palette's "no" red.
pub fn badge_no(ui: &mut Ui, text: &str) -> Response {
    badge(ui, text, theme::BAD)
}

/// A small neutral chip, matching `.hw-chip`: an accelerator name beside a number.
pub fn chip(ui: &mut Ui, text: &str) -> Response {
    let font = FontId::monospace(10.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, theme::TEXT_DIM);
    let pad = Vec2::new(6.0, 1.0);
    let (rect, response) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, Rounding::same(4.0), theme::BG_INSET);
        ui.painter().galley(rect.min + pad, galley, theme::TEXT_DIM);
    }
    response
}

/// Dimmed secondary text, matching `.sub`.
pub fn sub(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.label(RichText::new(text).size(12.0).color(theme::TEXT_DIM))
}

/// A paragraph of running prose, held to a readable measure.
pub fn prose(ui: &mut Ui, text: impl Into<String>) {
    ui.allocate_ui_with_layout(
        Vec2::new(theme::MEASURE.min(ui.available_width()), 0.0),
        Layout::top_down(Align::Min),
        |ui| {
            ui.label(RichText::new(text).color(theme::TEXT_DIM));
        },
    );
}

/// A raised card with a border, matching the `.card` / `.model-table` container.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(theme::BG_RAISED)
        .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
        .rounding(Rounding::same(10.0))
        .inner_margin(egui::Margin::same(14.0))
        .show(ui, add)
        .inner
}

/// A section heading, matching the `<h3>` the panels use.
pub fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(16.0).strong().color(theme::TEXT));
}

/// An estimated number: amber, prefixed with the tilde the whole project uses for "not measured".
pub fn estimate(ui: &mut Ui, value: f32) -> Response {
    ui.label(
        RichText::new(format!("~{value:.4}"))
            .monospace()
            .color(theme::ESTIMATE),
    )
}

/// A measured error rate: green, because a measurement is the thing worth trusting.
pub fn measured_rate(ui: &mut Ui, label: &str, rate: f32) -> Response {
    ui.label(
        RichText::new(format!("{label} {:.1}%", rate * 100.0))
            .monospace()
            .color(theme::GOOD),
    )
}
