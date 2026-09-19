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
mod tray;
mod widgets;

fn main() -> iced::Result {
    // A release build has no console, so without this every `tracing` call goes nowhere and a user
    // hitting a problem has nothing to send. The flush guard lives in `lw_app::logging`, because
    // the way this application quits runs no destructors.
    lw_app::logging::init();

    // Two copies would both hook the same keys, both open the microphone, and both type their
    // transcript into the focused window -- so the symptom of launching twice is every sentence
    // appearing twice. A second launch hands the window to the copy that is already running and
    // says nothing, because from the user's side their double-click simply worked.
    let _instance = match lw_platform::single_instance::claim("ai.localwisper.app", "LocalWisper") {
        lw_platform::single_instance::Claim::First(guard) => guard,
        lw_platform::single_instance::Claim::Already => {
            tracing::info!("another copy is already running; handing it the window");
            return Ok(());
        }
    };

    // Built here, on the main thread, before the event loop starts: the tray creates a hidden
    // window and that window's messages are pumped by whichever thread made it. `None` means this
    // machine would not give us one, and the window then becomes the only way back to the app.
    let tray = tray::Tray::new();

    iced::daemon(app::App::title, app::App::update, app::App::view)
        .theme(app::App::theme)
        .style(app::App::style)
        .subscription(app::App::subscription)
        .run_with(move || app::App::boot(tray))
}
