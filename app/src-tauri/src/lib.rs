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
pub mod measurements;
pub mod state;
pub mod worker;

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
/// Falling back to the default binding is a last resort for a settings file that bypassed
/// validation: `Settings::validate` rejects anything unregistrable (including modifiers-only
/// bindings) so the UI can report it, rather than letting the app listen on keys the user did
/// not choose.
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

/// Re-register the global shortcut if the binding changed, and announce the result either way.
///
/// The announcement is unconditional because `hotkey_changed` carries the *mode* as well as the
/// keys, and the mode changes far more often than the keys do. Returning early without emitting
/// left the UI showing the previous mode.
///
/// This function no longer touches the mode itself -- see [`AppState::apply_settings`], which is
/// where every mirrored setting is applied, and why.
pub fn reregister_shortcut(app: &AppHandle, settings: &Settings) -> Shortcut {
    let state = app.state::<AppState>();
    let next = shortcut_from_settings(settings);
    let current = state.shortcut();
    let registered = if current.as_ref() == Some(&next) {
        // Same keys: nothing to rebind, and re-registering would open a window where the hotkey
        // is dead. Report what the OS actually holds rather than assuming it took.
        state.shortcut_registered()
    } else {
        if let Some(old) = current {
            let _ = app.global_shortcut().unregister(old);
        }
        let ok = match app.global_shortcut().register(next) {
            Ok(()) => {
                tracing::info!("hotkey registered: {}", next.into_string());
                true
            }
            Err(e) => {
                tracing::error!("failed to register hotkey {}: {e}", next.into_string());
                false
            }
        };
        state.set_shortcut(Some(next), ok);
        ok
    };
    // The UI cannot rely on polling alone: the main webview exists before `setup` runs, so its
    // first `active_hotkey` call can land before the binding is in place. Announcing every
    // (re)registration means a frontend that asked too early is corrected, and one open while
    // the binding changes elsewhere (tray, external edit) stays accurate.
    let _ = app.emit(
        "hotkey_changed",
        serde_json::json!({
            "accelerator": registered.then(|| next.into_string()),
            "mode": settings.hotkey.mode,
        }),
    );
    next
}

/// Start file logging under `<app-data>/logs/`, returning the guard that must stay alive.
///
/// Without this every `tracing` call in the app is discarded: a release build has no console
/// (`windows_subsystem = "windows"`), so a user hitting a problem has nothing to send and no way
/// to see why, for instance, a hotkey failed to register.
///
/// **Nothing private is written.** No transcript text, audio, clipboard contents or keys are ever
/// passed to `tracing` (audited), and the default filter is `info`, which carries device and
/// backend facts only. `LW_LOG` raises it for debugging (e.g. `LW_LOG=debug`).
/// The log writer's flush guard.
///
/// Held here rather than in Tauri's managed state because [`quit`] must be able to *take* it: the
/// guard flushes on drop, and the exit path it uses does not run the appender's own shutdown.
static LOG_GUARD: std::sync::OnceLock<
    parking_lot::Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>,
> = std::sync::OnceLock::new();

fn init_logging(dir: &std::path::Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::EnvFilter;

    let logs = dir.join("logs");
    std::fs::create_dir_all(&logs).ok()?;
    let appender = tracing_appender::rolling::daily(&logs, "localwisper.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env("LW_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok()?;
    tracing::info!(
        "LocalWisper {} starting; logs in {}",
        env!("CARGO_PKG_VERSION"),
        logs.display()
    );
    Some(guard)
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
            commands::preview_sound,
            commands::list_sound_themes,
            commands::list_input_devices,
            commands::get_autostart,
            commands::set_autostart,
            catalog::get_capabilities,
            catalog::list_accelerators,
            catalog::active_backend,
            catalog::set_mic_test,
            catalog::list_models,
            catalog::install_model,
            catalog::cancel_install,
            catalog::run_benchmark,
            catalog::run_benchmark_all,
            catalog::local_measurements,
        ])
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            // Held for the process lifetime so the non-blocking writer flushes.
            if let Some(guard) = init_logging(&data_dir) {
                let slot = LOG_GUARD.get_or_init(|| parking_lot::Mutex::new(None));
                *slot.lock() = Some(guard);
            }
            let settings_path = data_dir.join("settings.json");
            let settings = Settings::load(&settings_path).unwrap_or_default();
            let worker = worker::spawn(app.handle().clone(), settings_path.clone());
            app.manage(AppState::new(
                settings_path,
                settings.overlay_enabled,
                settings.hotkey.mode,
                worker,
            ));

            {
                let state = app.state::<AppState>();
                state.apply_settings(&settings);
                // The OS owns the autostart registration; settings.json only mirrors it. If a user
                // removed the entry outside the app, believe the OS and write the file back.
                use tauri_plugin_autostart::ManagerExt;
                match app.autolaunch().is_enabled() {
                    Ok(actual) if actual != settings.autostart => {
                        tracing::info!("autostart: settings said {}, the OS says {actual}; following the OS", settings.autostart);
                        let mut s = settings.clone();
                        s.autostart = actual;
                        let _ = s.save(&state.settings_path);
                    }
                    Ok(_) => {}
                    Err(e) => tracing::debug!("could not read the autostart registration: {e}"),
                }
            }

            // Before the tray and the overlay: windows declared in tauri.conf.json already exist
            // by the time `setup` runs, so their webview can call `active_hotkey` at any moment.
            // Registering first shrinks that window; `hotkey_changed` closes it for good.
            reregister_shortcut(app.handle(), &settings);

            build_tray(app.handle())?;
            create_overlay_window(app.handle())?;
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

/// Quit the application.
///
/// Not `app.exit(0)`: once a WebGPU session has existed in the process, ONNX Runtime's WebGPU
/// execution provider crashes during library detach, so an ordinary exit ends in a crash dialog
/// rather than a quit (see `lw_ort::exit_without_teardown`). Logs are flushed first, because that
/// path does not run the appender's own shutdown.
fn quit(app: &AppHandle) {
    tracing::info!("quitting");
    // Dropping the non-blocking writer's guard is what flushes it; the state holds it.
    if let Some(slot) = LOG_GUARD.get() {
        drop(slot.lock().take()); // dropping the guard is what flushes the writer
    }
    let _ = app;
    std::thread::sleep(std::time::Duration::from_millis(80));
    lw_ort::exit_without_teardown(0);
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
            "quit" => quit(app),
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
            state.transition(app, RecordingState::Listening);
            state.worker.send(worker::WorkerCmd::StartRecording {
                hands_free: state.hotkey_mode() == HotkeyMode::HandsFree,
            });
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

#[cfg(test)]
mod tests {
    use super::*;
    use lw_core::settings::{HotkeyConfig, HotkeyMode};

    fn hotkey(modifiers: &[&str], trigger: &str) -> Settings {
        Settings {
            hotkey: HotkeyConfig {
                modifiers: modifiers.iter().map(|m| m.to_string()).collect(),
                trigger: trigger.into(),
                mode: HotkeyMode::PushToTalk,
            },
            ..Settings::default()
        }
    }

    #[test]
    fn the_default_binding_is_ctrl_alt_space() {
        assert_eq!(shortcut_from_settings(&Settings::default()), fallback_shortcut());
    }

    #[test]
    fn a_configured_binding_is_honoured() {
        assert_eq!(
            shortcut_from_settings(&hotkey(&["ctrl", "shift"], "d")),
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyD)
        );
        assert_eq!(
            shortcut_from_settings(&hotkey(&["alt"], "F9")),
            Shortcut::new(Some(Modifiers::ALT), Code::F9)
        );
    }

    #[test]
    fn a_binding_with_no_modifiers_still_registers() {
        assert_eq!(
            shortcut_from_settings(&hotkey(&[], "F13")),
            Shortcut::new(None, Code::F13)
        );
    }

    #[test]
    fn an_unregistrable_binding_falls_back_rather_than_leaving_no_hotkey() {
        // Only reachable from a hand-edited settings file: `Settings::validate` rejects both.
        assert_eq!(
            shortcut_from_settings(&hotkey(&["ctrl", "win"], "none")),
            fallback_shortcut()
        );
        assert_eq!(
            shortcut_from_settings(&hotkey(&["ctrl"], "wingding")),
            fallback_shortcut()
        );
    }

    #[test]
    fn validation_rejects_what_shortcut_resolution_cannot_register() {
        // The two must agree, or the UI would accept a binding the app then ignores.
        for (mods, trigger) in [
            (vec!["ctrl", "win"], "none"),
            (vec!["ctrl", "alt", "shift"], "none"),
            (vec!["ctrl"], "wingding"),
        ] {
            let s = hotkey(&mods, trigger);
            assert!(
                s.hotkey.validate().is_err(),
                "{mods:?}+{trigger} should be rejected"
            );
            assert_eq!(shortcut_from_settings(&s), fallback_shortcut());
        }
    }
}
