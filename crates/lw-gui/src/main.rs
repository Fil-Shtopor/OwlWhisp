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

mod app;
mod panels;
mod theme;
mod widgets;

fn main() -> iced::Result {
    // Held for the life of the process: the guard flushes the appender when it is dropped, and a
    // release build has no console, so without this every `tracing` call goes nowhere and a user
    // hitting a problem has nothing to send.
    let _log_guard = lw_app::logging::init();

    iced::application(app::App::title, app::App::update, app::App::view)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(1000.0, 720.0),
            min_size: Some(iced::Size::new(560.0, 420.0)),
            ..Default::default()
        })
        .run()
}
