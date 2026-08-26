//! Thin cross-platform clipboard wrapper over `arboard` (plain text get/set).
//!
//! The Windows text injector does **not** use this — it drives the Win32 clipboard
//! directly so it can attach the clipboard-history/cloud-sync exclusion formats and the
//! sequence-number restore guard. This wrapper is for straightforward "copy result to
//! clipboard" flows.

use crate::{Error, Result};

/// A clipboard handle. Cheap to create; hold one per operation or reuse.
pub struct Clipboard {
    inner: arboard::Clipboard,
}

impl Clipboard {
    /// Open the system clipboard.
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: arboard::Clipboard::new().map_err(|e| Error::Clipboard(e.to_string()))?,
        })
    }

    /// Current clipboard text, or `None` if the clipboard holds no text.
    pub fn get_text(&mut self) -> Result<Option<String>> {
        match self.inner.get_text() {
            Ok(t) => Ok(Some(t)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(Error::Clipboard(e.to_string())),
        }
    }

    /// Replace the clipboard contents with `text`.
    pub fn set_text(&mut self, text: &str) -> Result<()> {
        self.inner
            .set_text(text.to_string())
            .map_err(|e| Error::Clipboard(e.to_string()))
    }

    /// Clear the clipboard.
    pub fn clear(&mut self) -> Result<()> {
        self.inner.clear().map_err(|e| Error::Clipboard(e.to_string()))
    }
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Clipboard")
    }
}
