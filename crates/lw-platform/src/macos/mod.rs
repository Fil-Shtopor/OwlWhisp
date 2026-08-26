//! macOS placeholders.
//!
//! What already works on macOS through the shared cross-platform modules:
//! - audio capture (`crate::audio`, cpal → CoreAudio),
//! - clipboard (`crate::clipboard`, arboard → NSPasteboard),
//! - secrets (`crate::secrets`, keyring → Keychain),
//! - capability basics (`crate::caps::detect`, sysinfo).
//!
//! What is stubbed here because it needs macOS-only APIs (and a Mac to develop against):
//! - global hotkeys — needs a `CGEventTap` (kCGEventKeyDown/Up + flagsChanged for
//!   modifier-only combos) plus the Accessibility permission prompt;
//! - text injection — needs `CGEventPost` of Cmd+V and `CGEventKeyboardSetUnicodeString`
//!   for direct typing;
//! - foreground app — needs `NSWorkspace.frontmostApplication`.
//!
//! All stubs return [`Error::Unavailable`] with a message saying what is missing.

use crossbeam_channel::Receiver;

use crate::hotkey::{GlobalHotkey, HotkeyEvent, HotkeySpec};
use crate::inject::{ForegroundApp, TextInjector};
use crate::{Error, Result};

/// Placeholder hotkey listener. Registration always fails with `Unavailable`.
#[derive(Debug)]
pub struct MacosHotkey {
    // Kept so `events()` can hand out a receiver with the right type; nothing sends.
    channel: (crossbeam_channel::Sender<HotkeyEvent>, Receiver<HotkeyEvent>),
}

impl MacosHotkey {
    /// A new (non-functional) listener.
    pub fn new() -> Self {
        Self {
            channel: crossbeam_channel::bounded(1),
        }
    }
}

impl GlobalHotkey for MacosHotkey {
    fn register(&mut self, _spec: &HotkeySpec) -> Result<()> {
        // Unavailable: requires a CGEventTap; not implemented without a Mac to test on.
        Err(Error::Unavailable(
            "global hotkeys are not implemented on macOS yet (needs a CGEventTap)".into(),
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
pub struct MacosInjector;

impl TextInjector for MacosInjector {
    fn inject_into(&mut self, _text: &str, _restore_clipboard: bool) -> Result<()> {
        // Unavailable: requires CGEventPost(Cmd+V); not implemented without a Mac.
        Err(Error::Unavailable(
            "text injection is not implemented on macOS yet (needs CGEventPost Cmd+V)".into(),
        ))
    }

    fn type_unicode(&mut self, _text: &str) -> Result<()> {
        // Unavailable: requires CGEventKeyboardSetUnicodeString.
        Err(Error::Unavailable(
            "unicode typing is not implemented on macOS yet (needs CGEventKeyboardSetUnicodeString)".into(),
        ))
    }
}

/// Foreground app inspection is not implemented on macOS yet
/// (needs `NSWorkspace.frontmostApplication`).
pub fn foreground_app() -> Result<ForegroundApp> {
    Err(Error::Unavailable(
        "foreground app detection is not implemented on macOS yet".into(),
    ))
}
