//! Text injection into the focused application, and foreground-app inspection.
//!
//! The primary strategy is **clipboard paste**: stage the text on the clipboard (marked so
//! Windows excludes it from clipboard history / cloud sync), synthesize Ctrl+V, then restore
//! the previous clipboard **only if nothing else wrote to the clipboard in the meantime**
//! (guarded by the clipboard sequence number — the decision is the pure
//! [`should_restore`]). A direct-typing fallback
//! ([`type_unicode`](TextInjector::type_unicode)) synthesizes `KEYEVENTF_UNICODE` key
//! events, surrogate-pair aware.
//!
//! The Windows implementation lives in `crate::windows::WindowsInjector`; this module holds
//! the trait, the pure helpers, and their tests.

use crate::Result;

/// The foreground application, for profile matching (`lw_core::profiles`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForegroundApp {
    /// Executable file name (e.g. `"Code.exe"`), no directory.
    pub exe: String,
    /// Window title.
    pub title: String,
}

/// Platform-neutral text injection.
pub trait TextInjector: Send {
    /// Inject `text` into the focused control (clipboard-paste strategy), restoring the
    /// previous clipboard afterwards.
    fn inject(&mut self, text: &str) -> Result<()> {
        self.inject_into(text, true)
    }

    /// Inject `text` via clipboard paste. When `restore_clipboard` is false the text is
    /// left on the clipboard after pasting.
    fn inject_into(&mut self, text: &str, restore_clipboard: bool) -> Result<()>;

    /// Fallback: type `text` directly as synthetic Unicode key events (no clipboard).
    /// Slower, but leaves the clipboard untouched. Surrogate-pair aware.
    fn type_unicode(&mut self, text: &str) -> Result<()>;
}

/// The executable and window title of the current foreground application.
pub fn foreground_app() -> Result<ForegroundApp> {
    #[cfg(windows)]
    {
        crate::windows::foreground_app()
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::foreground_app()
    }
    #[cfg(target_os = "linux")]
    {
        crate::linux::foreground_app()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Err(crate::Error::Unavailable(
            "foreground app detection is not supported on this OS".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested; used by the per-OS implementations)
// ---------------------------------------------------------------------------

/// Restore-guard decision: restore the saved clipboard only when the clipboard sequence
/// number is exactly what it was right after we staged our text — i.e. nobody else wrote
/// to the clipboard since. A `sequence_after_set` of 0 means we never captured a valid
/// sequence (the counter itself is never 0 on success), so never restore.
pub fn should_restore(sequence_after_set: u32, sequence_now: u32) -> bool {
    sequence_after_set != 0 && sequence_after_set == sequence_now
}

/// Extract the bare executable file name from a full image path
/// (`C:\...\Code.exe` → `Code.exe`). Handles both `\` and `/` separators.
pub fn exe_name(image_path: &str) -> String {
    image_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(image_path)
        .to_string()
}

/// Normalize line endings to CRLF for `CF_UNICODETEXT` (Windows clipboard convention).
pub fn normalize_crlf(text: &str) -> String {
    // Collapse to \n first so existing \r\n does not double up.
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

/// UTF-16 code units for `text`, **without** a trailing NUL. Surrogate pairs come out as
/// two units, which is exactly what `KEYEVENTF_UNICODE` wants (one key event per unit).
pub fn utf16_units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// UTF-16 code units for `text` with a trailing NUL (clipboard payload form).
pub fn utf16_nul(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_guard_requires_exact_sequence_match() {
        assert!(should_restore(42, 42));
        assert!(!should_restore(42, 43)); // someone else wrote to the clipboard
        assert!(!should_restore(42, 41));
        assert!(!should_restore(0, 0)); // never captured a valid sequence
    }

    #[test]
    fn exe_name_strips_directories() {
        assert_eq!(
            exe_name(r"C:\Users\x\AppData\Local\Programs\Microsoft VS Code\Code.exe"),
            "Code.exe"
        );
        assert_eq!(exe_name("/usr/bin/kitty"), "kitty");
        assert_eq!(exe_name("standalone.exe"), "standalone.exe");
        assert_eq!(exe_name(""), "");
    }

    #[test]
    fn crlf_normalization_is_idempotent() {
        assert_eq!(normalize_crlf("a\nb\r\nc\rd"), "a\r\nb\r\nc\r\nd");
        assert_eq!(normalize_crlf("a\r\nb"), "a\r\nb");
        assert_eq!(normalize_crlf(&normalize_crlf("x\ny")), "x\r\ny");
    }

    #[test]
    fn utf16_handles_surrogate_pairs() {
        // U+1D11E MUSICAL SYMBOL G CLEF is outside the BMP: two UTF-16 units.
        let units = utf16_units("𝄞");
        assert_eq!(units.len(), 2);
        assert_eq!(units[0], 0xD834);
        assert_eq!(units[1], 0xDD1E);
        // BMP char: one unit.
        assert_eq!(utf16_units("é"), vec![0x00E9]);
    }

    #[test]
    fn utf16_nul_terminates() {
        let units = utf16_nul("hi");
        assert_eq!(units, vec![0x68, 0x69, 0]);
    }
}
