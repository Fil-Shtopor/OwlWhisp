//! Windows implementations: low-level keyboard hook hotkeys, clipboard-paste text
//! injection, foreground-app inspection, and registry/driver-store capability probes.

mod caps;
mod foreground;
mod hotkey;
mod inject;

pub use caps::{detect_npu, processor_name_from_registry};
pub use foreground::foreground_app;
pub use hotkey::WindowsHotkey;
pub use inject::WindowsInjector;

/// `dwExtraInfo` marker stamped on every key event we synthesize via `SendInput`, so the
/// low-level keyboard hook can tell our own injected keys from physical ones and ignore
/// them (otherwise the Ctrl of our Ctrl+V paste could perturb a Ctrl-based hotkey combo).
pub(crate) const LW_SENDINPUT_MARKER: usize = 0x4C57_5350; // "LWSP"
