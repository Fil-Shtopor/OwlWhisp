//! Overlay window and system tray **trait signatures only**.
//!
//! The real implementations are provided by the Tauri shell (`app/src-tauri`): the overlay
//! is a Tauri always-on-top webview and the tray uses `tauri::tray`. `lw-platform`
//! deliberately does **not** create raw OS windows. The no-op types here keep headless
//! consumers (CLI, tests) working against the same interfaces.

use crate::Result;

/// Visual state of the recording overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayState {
    /// Not visible / idle.
    #[default]
    Idle,
    /// Actively recording (level meter live).
    Recording,
    /// Transcribing / post-processing.
    Processing,
    /// Something went wrong; show an error affordance.
    Error,
}

/// A small always-on-top status overlay near the cursor or screen edge.
///
/// Implemented by the Tauri shell; [`NoopOverlay`] is the headless stand-in.
pub trait OverlayWindow: Send {
    /// Show the overlay.
    fn show(&mut self) -> Result<()>;
    /// Hide the overlay.
    fn hide(&mut self) -> Result<()>;
    /// Switch the visual state.
    fn set_state(&mut self, state: OverlayState) -> Result<()>;
    /// Feed the live microphone level (RMS in `[0, 1]`) for the meter.
    fn set_level(&mut self, rms: f32) -> Result<()>;
}

/// The system tray icon and menu.
///
/// Implemented by the Tauri shell; [`NoopTray`] is the headless stand-in.
pub trait SystemTray: Send {
    /// Update the tray tooltip.
    fn set_tooltip(&mut self, tooltip: &str) -> Result<()>;
    /// Reflect the app state in the tray icon.
    fn set_state(&mut self, state: OverlayState) -> Result<()>;
    /// Show a desktop notification.
    fn notify(&mut self, title: &str, body: &str) -> Result<()>;
}

/// Overlay that does nothing (headless/CLI use). All methods succeed.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopOverlay;

impl OverlayWindow for NoopOverlay {
    fn show(&mut self) -> Result<()> {
        Ok(())
    }
    fn hide(&mut self) -> Result<()> {
        Ok(())
    }
    fn set_state(&mut self, _state: OverlayState) -> Result<()> {
        Ok(())
    }
    fn set_level(&mut self, _rms: f32) -> Result<()> {
        Ok(())
    }
}

/// Tray that does nothing (headless/CLI use). All methods succeed.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopTray;

impl SystemTray for NoopTray {
    fn set_tooltip(&mut self, _tooltip: &str) -> Result<()> {
        Ok(())
    }
    fn set_state(&mut self, _state: OverlayState) -> Result<()> {
        Ok(())
    }
    fn notify(&mut self, _title: &str, _body: &str) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Making a real window behave like an overlay
// ---------------------------------------------------------------------------

/// Make an already-created window passive: it never takes focus, and clicks fall through it.
///
/// `hwnd` is the raw window handle, passed as an integer so this crate needs no window-toolkit
/// dependency to offer it.
///
/// A toolkit can usually manage "always on top" and "no decorations" by itself. It generally
/// cannot manage *not being activated*, and that is the property that matters most here: an
/// indicator that takes the keyboard focus when it appears interrupts the very sentence the user
/// is dictating into another program. `WS_EX_NOACTIVATE` is what stops that, and it has to be set
/// on the window before it is shown.
///
/// `WS_EX_TRANSPARENT` makes clicks fall through, and `WS_EX_TOOLWINDOW` keeps the window out of
/// alt-tab, where an indicator with no controls has nothing to offer.
///
/// This does not show or hide the window, deliberately. A window the toolkit believes is hidden
/// is a window it never asks anyone to paint, so a caller that hides it behind the toolkit's back
/// ends up with an overlay that exists, reports itself visible to the OS, and draws nothing.
#[cfg(windows)]
pub fn make_passive(hwnd: isize) -> crate::Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TRANSPARENT,
    };

    let hwnd = HWND(hwnd as *mut std::ffi::c_void);
    // SAFETY: a window handle owned by this process, read and written with the standard style
    // accessors. Both calls are infallible for a valid handle.
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if current == 0 {
            return Err(crate::Error::Unavailable(
                "could not read the overlay window's extended style".into(),
            ));
        }
        let wanted = current
            | (WS_EX_NOACTIVATE.0 as isize)
            | (WS_EX_TRANSPARENT.0 as isize)
            | (WS_EX_TOOLWINDOW.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
    }
    Ok(())
}

/// Not implemented away from Windows yet, so the overlay there may take focus when it opens.
#[cfg(not(windows))]
pub fn make_passive(_hwnd: isize) -> crate::Result<()> {
    Err(crate::Error::Unavailable(
        "making a window passive is only implemented on Windows".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noops_are_usable_through_the_traits() {
        let mut overlay: Box<dyn OverlayWindow> = Box::new(NoopOverlay);
        overlay.show().unwrap();
        overlay.set_state(OverlayState::Recording).unwrap();
        overlay.set_level(0.5).unwrap();
        overlay.hide().unwrap();

        let mut tray: Box<dyn SystemTray> = Box::new(NoopTray);
        tray.set_tooltip("LocalWisper").unwrap();
        tray.set_state(OverlayState::Idle).unwrap();
        tray.notify("done", "text pasted").unwrap();
    }
}
