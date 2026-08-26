//! LocalWisper desktop shell (Tauri 2).
//!
//! Responsibilities today:
//! - system tray (Settings / Diagnostics / Quit), main window hides to tray on close;
//! - a transparent, always-on-top, non-focusable, click-through overlay window
//!   (created hidden in Rust, shown while dictation is active);
//! - a global push-to-talk shortcut (Ctrl+Alt+Space) driving the recording-state
//!   machine and the `state_changed` event;
//! - IPC commands for settings (persisted with `lw_core::settings::Settings`),
//!   diagnostics (via `lw_ort::OrtRuntime`), and a mic-level channel;
//! - a background [`worker`] that owns the microphone capture and the Parakeet engine and turns
//!   hotkey press/release into capture → transcription → text pipeline → injection.

pub mod commands;
pub mod state;
pub mod worker;

use std::time::Duration;

use lw_core::settings::Settings;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutEvent, ShortcutState,
};

use crate::state::{AppState, RecordingState};

/// The default push-to-talk shortcut.
///
/// TODO: derive from `Settings.hotkey` (and re-register on settings change) once the
/// hotkey editor lands; for now the app-level default is fixed at Ctrl+Alt+Space.
fn dictation_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space)
}

/// Build and run the Tauri application.
pub fn run() {
    tauri::Builder::default()
        // single-instance MUST be the first plugin registered.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_log::Builder::new().level(log::LevelFilter::Info).build())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(on_shortcut).build())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::set_settings,
            commands::list_backends,
            commands::get_diagnostics,
            commands::get_recording_state,
            commands::set_recording_state,
            commands::subscribe_mic_level,
        ])
        .setup(|app| {
            let settings_path = app.path().app_data_dir()?.join("settings.json");
            let overlay_enabled =
                Settings::load(&settings_path).map(|s| s.overlay_enabled).unwrap_or(true);
            let worker = worker::spawn(app.handle().clone(), settings_path.clone());
            app.manage(AppState::new(settings_path, overlay_enabled, worker));

            build_tray(app.handle())?;
            create_overlay_window(app.handle())?;

            app.global_shortcut().register(dictation_shortcut())?;
            Ok(())
        })
        // Closing the main window hides it to the tray; "Quit" in the tray exits.
        .on_window_event(|window, event| {
            if window.label() == "main"
                && let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
        })
        .run(tauri::generate_context!())
        .expect("failed to run LocalWisper");
}

/// System tray with Settings / Diagnostics / Quit.
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let settings_item = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let diagnostics_item = MenuItem::with_id(app, "diagnostics", "Diagnostics", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&settings_item, &diagnostics_item, &quit_item])?;

    let mut tray = TrayIconBuilder::with_id("main-tray")
        .tooltip("LocalWisper")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "settings" => show_main_window(app, Some("settings")),
            "diagnostics" => show_main_window(app, Some("diagnostics")),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Show + focus the main window; optionally tell the frontend to switch tab
/// (the frontend listens for `open_tab`).
fn show_main_window(app: &AppHandle, tab: Option<&str>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
    if let Some(tab) = tab {
        let _ = app.emit("open_tab", tab);
    }
}

/// The recording pill: transparent, undecorated, always-on-top, skip-taskbar,
/// non-focusable, click-through. Created hidden; `AppState::sync_overlay` shows it
/// while dictation is active.
fn create_overlay_window(app: &AppHandle) -> tauri::Result<()> {
    let overlay = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
        .title("LocalWisper Overlay")
        .inner_size(240.0, 56.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focusable(false)
        .focused(false)
        .visible(false)
        .build()?;

    // Never steal clicks from the app underneath.
    let _ = overlay.set_ignore_cursor_events(true);

    // Park it bottom-center of the primary monitor.
    if let Ok(Some(monitor)) = overlay.primary_monitor()
        && let Ok(size) = overlay.outer_size()
    {
        let mpos = monitor.position();
        let msize = monitor.size();
        let x = mpos.x + (msize.width as i32 - size.width as i32) / 2;
        let y = mpos.y + msize.height as i32 - size.height as i32 - 72;
        let _ = overlay.set_position(tauri::PhysicalPosition::new(x, y));
    }
    Ok(())
}

/// Global-shortcut handler: push-to-talk transitions of the recording-state machine.
///
/// Pressed starts microphone capture in the [`worker`]; Released stops capture and asks the worker
/// to transcribe and deliver the text. The worker emits the `processing → done → idle` transitions
/// (and `transcript` / `worker_error`) itself when it finishes.
fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if *shortcut != dictation_shortcut() {
        return;
    }
    let state = app.state::<AppState>();
    match event.state() {
        ShortcutState::Pressed => {
            if state.recording_state() == RecordingState::Listening {
                return; // key auto-repeat
            }
            let generation = state.transition(app, RecordingState::Listening);
            state.worker.send(worker::WorkerCmd::StartRecording);
            spawn_mic_level_stub(app.clone(), generation);
        }
        ShortcutState::Released => {
            if state.recording_state() != RecordingState::Listening {
                return;
            }
            // Enter Processing; the worker stops capture, transcribes, injects, then emits the
            // `done` -> `idle` state transitions itself when finished (see worker.rs).
            state.transition(app, RecordingState::Processing);
            state.worker.send(worker::WorkerCmd::StopRecording);
        }
    }
}

/// Feed the `mic_level` channel with synthetic levels while listening.
///
/// TODO: wire to lw-platform AudioCapture — replace with real RMS/peak levels from
/// the capture callback.
fn spawn_mic_level_stub(app: AppHandle, generation: u64) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let mut t = 0f32;
        while state.generation() == generation && state.recording_state() == RecordingState::Listening {
            let level = (0.18 + 0.6 * ((t * 2.3).sin().abs() * (t * 0.9).cos().abs())).min(1.0);
            if let Some(channel) = state.mic_level.lock().as_ref() {
                let _ = channel.send(level);
            }
            t += 0.05;
            std::thread::sleep(Duration::from_millis(50));
        }
        if let Some(channel) = state.mic_level.lock().as_ref() {
            let _ = channel.send(0.0);
        }
    });
}
