//! # lw-platform
//!
//! OS integration for OwlWhisp: microphone capture, global hotkeys, text injection,
//! clipboard access, foreground-app inspection, hardware-capability detection, and secret
//! storage.
//!
//! The crate defines **platform-neutral traits** in the top-level modules and implements them
//! per operating system behind `#[cfg(...)]`:
//!
//! - [`windows`] — full implementations (low-level keyboard hook, clipboard paste injection,
//!   Qualcomm NPU detection, foreground app).
//! - [`macos`] — placeholder implementations that return [`Error::Unavailable`]; audio capture,
//!   clipboard, secrets, and capability detection work through the shared cross-platform paths.
//! - [`linux`] — compiling stubs returning [`Error::Unavailable`].
//!
//! Use [`platform()`] to obtain the per-OS entry point, or construct the concrete types
//! directly ([`audio::Capture`], `windows::WindowsHotkey`, ...).
//!
//! Overlay windows and the system tray are **not** implemented here — the Tauri shell provides
//! them. [`overlay`] only defines the trait signatures plus no-op defaults.
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod audio;
pub mod autostart;
pub mod browser;
pub mod caps;
pub mod clipboard;
pub mod drivers;
pub mod hotkey;
pub mod inject;
pub mod overlay;
pub mod screen;
pub mod secrets;
pub mod single_instance;
pub mod sound;
pub mod updates;

#[cfg(windows)]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

pub use audio::{AudioCapture, Capture, LevelHandle};
pub use clipboard::Clipboard;
pub use hotkey::{Captured, GlobalHotkey, HotkeyCapture, HotkeyEvent, HotkeySpec, RawCombo};
pub use inject::{ForegroundApp, TextInjector};
pub use overlay::{NoopOverlay, NoopTray, OverlayWindow, SystemTray};
pub use secrets::SecureStore;
pub use sound::play as play_cue;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced by `lw-platform`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Audio device / stream failure.
    #[error("audio: {0}")]
    Audio(String),

    /// Hotkey registration or hook failure.
    #[error("hotkey: {0}")]
    Hotkey(String),

    /// Text injection (clipboard paste / synthetic keys) failure.
    #[error("inject: {0}")]
    Inject(String),

    /// Clipboard access failure.
    #[error("clipboard: {0}")]
    Clipboard(String),

    /// Secure secret-store failure.
    #[error("secrets: {0}")]
    Secrets(String),

    /// A feature is not available on this OS (or not implemented yet).
    #[error("unavailable: {0}")]
    Unavailable(String),

    /// An error bubbled up from `lw-core` (e.g. resampling).
    #[error(transparent)]
    Core(#[from] lw_core::Error),
}

/// Entry point bundling the per-OS factory functions.
///
/// Obtain one with [`platform()`]. All methods are thin dispatchers; the concrete types are
/// also public if you prefer to construct them directly.
#[derive(Clone, Copy, Debug, Default)]
pub struct Platform;

/// The per-OS platform accessor.
pub fn platform() -> Platform {
    Platform
}

impl Platform {
    /// Human-readable names of the available audio input devices.
    pub fn list_input_devices(&self) -> Vec<String> {
        audio::list_input_devices()
    }

    /// A microphone capture handle. `device` selects an input device by (sub)name;
    /// `None` uses the system default. `ring_capacity_samples` bounds the rolling
    /// capture window (native-rate samples).
    pub fn capture(&self, device: Option<String>, ring_capacity_samples: usize) -> Capture {
        Capture::new(device, ring_capacity_samples)
    }

    /// The global-hotkey listener for this OS.
    ///
    /// Windows: low-level keyboard hook. macOS/Linux: [`Error::Unavailable`].
    pub fn hotkeys(&self) -> Result<Box<dyn GlobalHotkey>> {
        #[cfg(windows)]
        {
            Ok(Box::new(windows::WindowsHotkey::new()?))
        }
        #[cfg(target_os = "macos")]
        {
            Err(Error::Unavailable(
                "global hotkeys are not implemented on macOS yet (needs a CGEventTap)".into(),
            ))
        }
        #[cfg(target_os = "linux")]
        {
            Err(Error::Unavailable(
                "global hotkeys are not implemented on Linux yet".into(),
            ))
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            Err(Error::Unavailable(
                "global hotkeys are not supported on this OS".into(),
            ))
        }
    }

    /// Listen for the next combination the user presses, for the shortcut editor.
    ///
    /// Not the same listener as [`hotkeys`](Platform::hotkeys), and not the same thing as reading
    /// the window's own key events. A window sees what the shell has left for it, and the shell
    /// keeps most of what the Windows key is part of -- which is why capture used to be unable to
    /// read a binding the hotkey backend would have been perfectly happy to detect.
    ///
    /// The session takes every key while it lasts, so that arming it cannot open the Start menu
    /// or leave a stray character in the field behind the window. Drop the returned box to stop.
    ///
    /// Windows: the process-wide keyboard hook. macOS/Linux: [`Error::Unavailable`], and callers
    /// are expected to fall back to their own window's key events.
    pub fn hotkey_capture(&self) -> Result<Box<dyn HotkeyCapture>> {
        #[cfg(windows)]
        {
            Ok(Box::new(windows::WindowsCapture::new()?))
        }
        #[cfg(not(windows))]
        {
            Err(Error::Unavailable(
                "reading a shortcut off the keyboard needs a system-wide keyboard hook, \
                 which only the Windows backend has"
                    .into(),
            ))
        }
    }

    /// How many key transitions the system-wide keyboard hook has seen, if this OS has one.
    ///
    /// `Some(0)` after the machine has been typed on is the one fact that separates two faults
    /// that look identical from the outside: a shortcut that does nothing because the binding is
    /// wrong, and a shortcut that does nothing because the hook this process installed is not
    /// being called at all. Nothing else in the application can tell them apart, so the number is
    /// put where a user can read it out.
    pub fn keyboard_hook_keys_seen(&self) -> Option<u64> {
        #[cfg(windows)]
        {
            Some(windows::keyboard_hook_key_counts().0)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// The text injector for this OS.
    ///
    /// Windows: clipboard-paste with Ctrl+V synthesis and guarded restore.
    /// macOS/Linux: [`Error::Unavailable`].
    pub fn injector(&self) -> Result<Box<dyn TextInjector>> {
        #[cfg(windows)]
        {
            Ok(Box::new(windows::WindowsInjector::new()))
        }
        #[cfg(target_os = "macos")]
        {
            Err(Error::Unavailable(
                "text injection is not implemented on macOS yet (needs CGEventPost Cmd+V)".into(),
            ))
        }
        #[cfg(target_os = "linux")]
        {
            Err(Error::Unavailable(
                "text injection is not implemented on Linux yet".into(),
            ))
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            Err(Error::Unavailable(
                "text injection is not supported on this OS".into(),
            ))
        }
    }

    /// A clipboard handle (cross-platform, via `arboard`).
    pub fn clipboard(&self) -> Result<Clipboard> {
        Clipboard::new()
    }

    /// Detect hardware/OS capabilities. `providers.*` is left for `lw-ort` to fill.
    pub fn capabilities(&self) -> lw_core::capabilities::Capabilities {
        caps::detect()
    }

    /// A secure secret store scoped to `service` (cross-platform, via `keyring`).
    pub fn secure_store(&self, service: impl Into<String>) -> SecureStore {
        SecureStore::new(service)
    }

    /// The executable and window title of the current foreground application.
    ///
    /// Windows: fully implemented. macOS/Linux: [`Error::Unavailable`].
    pub fn foreground_app(&self) -> Result<ForegroundApp> {
        inject::foreground_app()
    }

    /// Whether the window in front belongs to this process. See [`inject::foreground_is_own_process`].
    pub fn foreground_is_own_process(&self) -> bool {
        inject::foreground_is_own_process()
    }
}

/// The crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
