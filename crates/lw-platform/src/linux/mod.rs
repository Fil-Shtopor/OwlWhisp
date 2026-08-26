//! Linux stubs. Audio (cpal), clipboard (arboard), secrets (keyring), and capability
//! basics (sysinfo) work through the shared cross-platform modules; hotkeys, text
//! injection, and foreground-app detection are not implemented (X11 vs Wayland split —
//! evdev/XGrabKey/xdg-desktop-portal are all viable seams) and return
//! [`Error::Unavailable`].

use crossbeam_channel::Receiver;

use crate::hotkey::{GlobalHotkey, HotkeyEvent, HotkeySpec};
use crate::inject::{ForegroundApp, TextInjector};
use crate::{Error, Result};

/// Placeholder hotkey listener. Registration always fails with `Unavailable`.
#[derive(Debug)]
pub struct LinuxHotkey {
    channel: (crossbeam_channel::Sender<HotkeyEvent>, Receiver<HotkeyEvent>),
}

impl LinuxHotkey {
    /// A new (non-functional) listener.
    pub fn new() -> Self {
        Self {
            channel: crossbeam_channel::bounded(1),
        }
    }
}

impl GlobalHotkey for LinuxHotkey {
    fn register(&mut self, _spec: &HotkeySpec) -> Result<()> {
        Err(Error::Unavailable(
            "global hotkeys are not implemented on Linux yet".into(),
        ))
    }

    fn unregister(&mut self) -> Result<()> {
        Ok(())
    }

    fn events(&self) -> Receiver<HotkeyEvent> {
        self.channel.1.clone()
    }
}

/// Placeholder text injector. All methods fail with `Unavailable`.
#[derive(Clone, Copy, Debug, Default)]
pub struct LinuxInjector;

impl TextInjector for LinuxInjector {
    fn inject_into(&mut self, _text: &str, _restore_clipboard: bool) -> Result<()> {
        Err(Error::Unavailable(
            "text injection is not implemented on Linux yet".into(),
        ))
    }

    fn type_unicode(&mut self, _text: &str) -> Result<()> {
        Err(Error::Unavailable(
            "unicode typing is not implemented on Linux yet".into(),
        ))
    }
}

/// Foreground app inspection is not implemented on Linux yet.
pub fn foreground_app() -> Result<ForegroundApp> {
    Err(Error::Unavailable(
        "foreground app detection is not implemented on Linux yet".into(),
    ))
}
