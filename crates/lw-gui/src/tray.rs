//! The system tray icon and its menu.
//!
//! The application is meant to sit in the background and type into other programs, so the tray is
//! not a convenience: it is where the application lives once the window is closed. The menu is the
//! one the Tauri build had -- Settings, Diagnostics, Quit -- because those are the two places a
//! user goes when something is wrong, plus the way out.
//!
//! `tray-icon` publishes menu clicks on a global channel rather than through a handle, so the
//! window polls it. That is the same shape as the dictation worker's channel and costs a `try_recv`
//! per frame.

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

/// What the user picked in the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Show the window on the Settings tab.
    OpenSettings,
    /// Show the window on the Diagnostics tab.
    OpenDiagnostics,
    /// Leave.
    Quit,
}

/// The 128x128 application icon, the same one the Tauri build used.
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/128x128.png");

/// A live tray icon. Dropping it removes the icon from the tray.
pub struct Tray {
    /// Held so the icon stays in the tray; nothing is read back from it.
    _icon: TrayIcon,
    settings: tray_icon::menu::MenuId,
    diagnostics: tray_icon::menu::MenuId,
    quit: tray_icon::menu::MenuId,
}

impl Tray {
    /// Put the icon in the tray.
    ///
    /// Must be called on the thread that pumps window messages -- the main thread, before iced's
    /// event loop starts -- because that is where the hidden window `tray-icon` creates lives.
    ///
    /// Returns `None` if the tray refuses us, which happens on a machine with the notification
    /// area locked down. The application still works; it simply has no tray, and the window
    /// becomes the only way back to it, so the caller must not close to a tray that is not there.
    pub fn new() -> Option<Self> {
        let settings = MenuItem::new("Settings", true, None);
        let diagnostics = MenuItem::new("Diagnostics", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&settings, &diagnostics, &quit]).ok()?;

        let icon = load_icon()?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("OwlWhisp")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(true)
            .with_icon(icon)
            .build()
            .map_err(|e| tracing::warn!("no tray icon: {e}"))
            .ok()?;

        Some(Self {
            _icon: tray,
            settings: settings.id().clone(),
            diagnostics: diagnostics.id().clone(),
            quit: quit.id().clone(),
        })
    }

    /// Drain the menu channel. Returns the actions clicked since the last call.
    ///
    /// The channel is global to the process, so an event for an item we do not know about is
    /// ignored rather than guessed at.
    pub fn poll(&self) -> Vec<Action> {
        let mut out = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.settings {
                out.push(Action::OpenSettings);
            } else if event.id == self.diagnostics {
                out.push(Action::OpenDiagnostics);
            } else if event.id == self.quit {
                out.push(Action::Quit);
            }
        }
        out
    }
}

/// The same icon, as the window toolkit wants it: for the title bar, the taskbar and alt-tab.
pub fn window_icon() -> Option<iced::window::Icon> {
    let (rgba, width, height) = decode()?;
    iced::window::icon::from_rgba(rgba, width, height).ok()
}

/// Decode the embedded PNG into the RGBA both icons want.
fn load_icon() -> Option<tray_icon::Icon> {
    let (rgba, width, height) = decode()?;
    tray_icon::Icon::from_rgba(rgba, width, height)
        .map_err(|e| tracing::warn!("the tray icon was rejected: {e}"))
        .ok()
}

fn decode() -> Option<(Vec<u8>, u32, u32)> {
    let decoded = image::load_from_memory(ICON_PNG)
        .map_err(|e| tracing::warn!("the application icon did not decode: {e}"))
        .ok()?
        .into_rgba8();
    let (width, height) = decoded.dimensions();
    Some((decoded.into_raw(), width, height))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_embedded_icon_decodes_to_a_square_the_tray_will_take() {
        // A tray icon that fails to decode leaves the application with no way back once its window
        // is closed, and the failure is invisible until someone closes the window.
        let icon = image::load_from_memory(super::ICON_PNG).expect("decode");
        let rgba = icon.into_rgba8();
        assert_eq!(rgba.dimensions(), (128, 128));
        assert!(super::load_icon().is_some(), "the tray rejected it");
        assert!(super::window_icon().is_some(), "the window rejected it");
    }
}
