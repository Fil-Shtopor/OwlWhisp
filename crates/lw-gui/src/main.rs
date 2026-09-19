// No console window in a release build. A double-clicked GUI application that also opens a black
// terminal behind itself looks broken, and the Tauri build did not do it either.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The LocalWisper window, drawn natively on the CPU.
//!
//! Replaces the Tauri/WebView2 front end. Measured on the target machine: that one costs eight
//! processes and 240 MB of private commit to show a settings form and a table; an equivalent
//! window with a GPU context costs 113 MB in one process; this one costs 18 MB, because nothing
//! here opens a GPU context at all. The rasteriser is tiny-skia, which also means the same pixels
//! on Windows, macOS and Linux rather than whatever each platform's driver produces.
//!
//! A `daemon` rather than an `application`, because there are two windows: the main one and the
//! dictation overlay, which is opened and closed as dictation starts and stops. A daemon has no
//! window of its own until one is asked for, so the main window is opened by the initial task and
//! the process exits when it closes.

mod app;
mod panels;
mod theme;
mod widgets;

fn main() -> iced::Result {
    // Held for the life of the process: the guard flushes the appender when it is dropped, and a
    // release build has no console, so without this every `tracing` call goes nowhere and a user
    // hitting a problem has nothing to send.
    let _log_guard = lw_app::logging::init();

    iced::daemon(app::App::title, app::App::update, app::App::view)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .run_with(app::App::boot)
}
