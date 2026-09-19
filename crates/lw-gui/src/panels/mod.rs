//! One module per tab, mirroring `app/frontend/src/panels/`.

pub mod benchmark;
pub mod diagnostics;
pub mod dictate;
pub mod models;
pub mod settings;

/// Shown by a tab that has not been ported yet.
///
/// Deliberately blunt. A half-drawn panel that looks finished is worse than an empty one that says
/// what it is: the whole point of porting tab by tab is that the old window stays available while
/// this one catches up, and a user has to be able to tell which they are looking at.
pub fn not_yet(ui: &mut egui::Ui, what: &str) {
    crate::widgets::card(ui, |ui| {
        crate::widgets::heading(ui, what);
        crate::widgets::prose(
            ui,
            "Not ported to the native window yet. This tab still works in the Tauri build; it is \
             being moved one panel at a time so the application keeps running throughout.",
        );
    });
}
