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

pub mod catalog;
pub mod commands;
pub mod state;
pub mod worker;

use std::time::Duration;

use lw_core::settings::{HotkeyMode, Settings};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutEvent, ShortcutState,
};

use crate::state::{AppState, RecordingState};

/// The fallback binding used when settings are missing or hold an unregistrable combination.
fn fallback_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space)
}

/// Resolve the dictation shortcut from settings.
///
/// A modifiers-only binding (e.g. Ctrl+Win) cannot be registered through the OS shortcut API — it
/// needs the low-level keyboard hook in `lw-platform` — so we fall back to the default binding and
/// say so in the log rather than silently having no hotkey.
fn shortcut_from_settings(settings: &Settings) -> Shortcut {
    match settings.hotkey.to_accelerator() {
        Some(accel) => match accel.parse::<Shortcut>() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("hotkey '{accel}' is not a valid accelerator ({e}); using the default");
                fallback_shortcut()
            }
        },
        None => {
            // `Settings::validate` rejects modifier-only bindings precisely so this cannot be
            // reached from the UI; only a hand-edited settings file gets here.
            tracing::warn!(
                "hotkey {:?}+{} cannot be registered with the OS; using the default binding",
                settings.hotkey.modifiers,
                settings.hotkey.trigger
            );
            fallback_shortcut()
        }
    }
}

/// Re-register the global shortcut after the binding changed. Returns the shortcut now in effect.
pub fn reregister_shortcut(app: &AppHandle, settings: &Settings) -> Shortcut {
    let state = app.state::<AppState>();
    let next = shortcut_from_settings(settings);
    let current = state.shortcut();
    if current.as_ref() == Some(&next) {
        return next;
    }
    if let Some(old) = current {
        let _ = app.global_shortcut().unregister(old);
    }
    if let Err(e) = app.global_shortcut().register(next) {
        tracing::error!("failed to register hotkey: {e}");
    }
    state.set_shortcut(Some(next));
    state.set_hotkey_mode(settings.hotkey.mode);
    next
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
            commands::active_hotkey,
            catalog::get_capabilities,
            catalog::list_models,
            catalog::install_model,
            catalog::cancel_install,
            catalog::run_benchmark,
        ])
        .setup(|app| {
            let settings_path = app.path().app_data_dir()?.join("settings.json");
            let settings = Settings::load(&settings_path).unwrap_or_default();
            let worker = worker::spawn(app.handle().clone(), settings_path.clone());
            app.manage(AppState::new(
                settings_path,
                settings.overlay_enabled,
                settings.hotkey.mode,
                worker,
            ));

            build_tray(app.handle())?;
            create_overlay_window(app.handle())?;

            reregister_shortcut(app.handle(), &settings);
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

/// Global-shortcut handler. Three binding behaviours, chosen by `Settings.hotkey.mode`:
///
/// - **PushToTalk** — press starts capture, release stops it and transcribes.
/// - **Toggle** — press starts capture, the *next* press stops it; key release is ignored, so the
///   keys do not have to be held down.
/// - **HandsFree** — press starts capture; the worker's voice-activity detector ends the utterance
///   on silence (the same press also stops it early).
///
/// The worker emits the `processing → done → idle` transitions and `transcript`/`worker_error`.
fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    let state = app.state::<AppState>();
    if state.shortcut().as_ref() != Some(shortcut) {
        return;
    }
    let hold_to_talk = state.hotkey_mode() == HotkeyMode::PushToTalk;
    match event.state() {
        ShortcutState::Pressed => {
            if state.recording_state() == RecordingState::Listening {
                // Hold-to-talk sees key auto-repeat here and must ignore it; the tap modes treat a
                // second press as "stop".
                if hold_to_talk {
                    return;
                }
                stop_recording(app, &state);
                return;
            }
            if state.recording_state() == RecordingState::Processing {
                return; // still finishing the previous utterance
            }
            let generation = state.transition(app, RecordingState::Listening);
            state.worker.send(worker::WorkerCmd::StartRecording {
                hands_free: state.hotkey_mode() == HotkeyMode::HandsFree,
            });
            spawn_mic_level_stub(app.clone(), generation);
        }
        ShortcutState::Released => {
            if !hold_to_talk || state.recording_state() != RecordingState::Listening {
                return;
            }
            stop_recording(app, &state);
        }
    }
}

/// Move to `Processing` and ask the worker to finish the utterance.
fn stop_recording(app: &AppHandle, state: &AppState) {
    state.transition(app, RecordingState::Processing);
    state.worker.send(worker::WorkerCmd::StopRecording);
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
