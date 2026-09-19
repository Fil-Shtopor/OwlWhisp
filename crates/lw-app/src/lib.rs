//! Everything a LocalWisper front end needs that is not drawing.
//!
//! This crate exists because the same logic had been living inside the Tauri command layer, where
//! it could only be reached through an IPC call from a webview. That was fine while there was one
//! front end. It stopped being fine the moment a second one was wanted: the rules for which model
//! to suggest, what a benchmark means, and which accelerators an entry can use are decisions about
//! data, and a decision about data does not belong in whichever toolkit happens to draw the window.
//!
//! The split is: `lw-core` knows about models, engines and audio; this crate knows about the
//! application -- settings, the catalog as a user sees it, installs, benchmark runs and the
//! measurements this machine has taken; a front end knows about pixels and nothing else.
//!
//! Nothing here mentions Tauri, egui, or any other toolkit, and nothing here returns
//! `serde_json::Value` at an API boundary: a front end gets typed structs, because the last time
//! types were mirrored by hand across a boundary they drifted and printed `NaN%` at a user.

pub mod bench;
pub mod catalog;
pub mod machine;
pub mod measurements;

pub use catalog::{AcceleratorView, CatalogView, EntryView, RoleView};
pub use machine::probe_capabilities;
pub use measurements::{LocalMeasurement, LocalMeasurements};
