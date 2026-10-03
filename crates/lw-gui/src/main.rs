// No console window in a release build. A double-clicked GUI application that also opens a black
// terminal behind itself looks broken, and the Tauri build did not do it either.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The OwlWhisp window, drawn natively on the CPU.
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
mod smooth;
mod theme;
mod tray;
mod widgets;

fn main() -> iced::Result {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--diagnose-accelerators")
    {
        let report = lw_app::diagnostics::collect(env!("CARGO_PKG_VERSION"));
        println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
        return Ok(());
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--provider-probe")
    {
        let runtime_dir = std::env::args_os().nth(2).map(std::path::PathBuf::from);
        let result = runtime_dir
            .ok_or_else(|| "missing runtime directory".to_string())
            .and_then(|dir| {
                lw_app::provider_worker::run_provider_probe(
                    &dir,
                    std::env::args()
                        .nth(3)
                        .and_then(|id| lw_core::capabilities::Accelerator::from_id(&id))
                        .unwrap_or(lw_core::capabilities::Accelerator::DirectMl),
                )
            });
        println!("{}", serde_json::to_string(&result).unwrap_or_default());
        lw_ort::exit_without_teardown(i32::from(result.is_err()));
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--provider-worker")
    {
        if let Err(error) = lw_app::provider_worker::run_provider_worker() {
            eprintln!("Provider worker: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--install-runtime")
    {
        let result = std::env::args()
            .nth(2)
            .and_then(|id| lw_core::capabilities::Accelerator::from_id(&id))
            .ok_or_else(|| "missing or invalid accelerator".to_string())
            .and_then(|accel| {
                let handle = lw_app::runtime_install::start(accel);
                loop {
                    if let Some(result) = handle.poll() {
                        break result.map(|p| p.display().to_string());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });
        println!("{}", serde_json::to_string(&result).unwrap_or_default());
        if result.is_err() {
            std::process::exit(1);
        }
        return Ok(());
    }
    // A release build has no console, so without this every `tracing` call goes nowhere and a user
    // hitting a problem has nothing to send. The flush guard lives in `lw_app::logging`, because
    // the way this application quits runs no destructors.
    lw_app::logging::init();

    // Two copies would both hook the same keys, both open the microphone, and both type their
    // transcript into the focused window -- so the symptom of launching twice is every sentence
    // appearing twice. A second launch hands the window to the copy that is already running and
    // says nothing, because from the user's side their double-click simply worked.
    let _instance = match lw_platform::single_instance::claim("ai.owlwhisp.app", "OwlWhisp") {
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

    // A `Cell` because iced 0.14 takes the boot function as an `Fn`, and the tray can only be
    // handed over once. It is called exactly once; a second call would boot without a tray, which
    // is the correct answer to a question that is never asked.
    let tray = std::cell::Cell::new(tray);
    iced::daemon(
        move || app::App::boot(tray.take()),
        app::App::update,
        app::App::view,
    )
    .title(app::App::title)
    .theme(app::App::theme)
    .style(app::App::style)
    .subscription(app::App::subscription)
    .run()
}
