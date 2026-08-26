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
