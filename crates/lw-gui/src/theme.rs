//! The palette and spacing of the window, carried over from the web front end it replaces.
//!
//! The colours are the ones in `app/frontend/src/styles.css`, token for token, so the native
//! window is recognisably the same application rather than a different one that does the same
//! job. What cannot be carried over is the type: the web build asked for "Segoe UI Variable Text"
//! and got hinting and subpixel positioning from the browser's text stack. egui rasterises its own
//! glyphs, so text will be close but not identical. That is stated here rather than discovered.

use egui::{Color32, Rounding, Stroke, Visuals};

/// `--bg`
pub const BG: Color32 = Color32::from_rgb(0x11, 0x13, 0x1a);
/// `--bg-raised`
pub const BG_RAISED: Color32 = Color32::from_rgb(0x1a, 0x1d, 0x27);
/// `--bg-inset`
pub const BG_INSET: Color32 = Color32::from_rgb(0x0c, 0x0e, 0x14);
/// `--border`
pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x2e, 0x3d);
/// `--text`
pub const TEXT: Color32 = Color32::from_rgb(0xe5, 0xe7, 0xee);
/// `--text-dim`
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9a, 0xa0, 0xb0);
/// `--accent`
pub const ACCENT: Color32 = Color32::from_rgb(0x89, 0xb4, 0xfa);

/// `.badge.yes` — a measurement that exists, a capability that is real.
pub const GOOD: Color32 = Color32::from_rgb(0x22, 0xc5, 0x5e);
/// `.badge.no` — cannot run, refused, absent.
pub const BAD: Color32 = Color32::from_rgb(0xef, 0x44, 0x44);
/// The amber used for estimates, which must never look like measurements.
pub const ESTIMATE: Color32 = Color32::from_rgb(0xf5, 0xa9, 0x7a);

/// The widest a paragraph of running prose is allowed to get, in points.
///
/// `--measure: 78ch` in the stylesheet, and for the same reason: a sentence two thousand pixels
/// wide is measurably harder to read because the eye loses the line on the way back. Everything
/// else in the window grows with it.
pub const MEASURE: f32 = 620.0;

/// Apply the palette to a context. Called once at startup.
pub fn install(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BG;
    visuals.window_fill = BG_RAISED;
    visuals.extreme_bg_color = BG_INSET;
    visuals.faint_bg_color = BG_RAISED;
    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = ACCENT.linear_multiply(0.25);
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);

    let r = Rounding::same(8.0);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.rounding = r;
        w.bg_stroke = Stroke::new(1.0_f32, BORDER);
    }
    visuals.widgets.noninteractive.bg_fill = BG_RAISED;
    visuals.widgets.inactive.bg_fill = BG_RAISED;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x22, 0x26, 0x33);
    visuals.widgets.active.bg_fill = Color32::from_rgb(0x2a, 0x2f, 0x3f);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(16.0);
    ctx.set_style(style);
}
