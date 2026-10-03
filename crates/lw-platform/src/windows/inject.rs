//! Windows text injection: clipboard-paste with guarded restore, plus a direct
//! Unicode-typing fallback.
//!
//! Paste flow:
//! 1. Snapshot the current `CF_UNICODETEXT` (text-only snapshot — non-text formats are not
//!    preserved; see the TODO on [`WindowsInjector`]).
//! 2. Stage our text with the Windows 11 exclusion formats
//!    (`ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory` = 0,
//!    `CanUploadToCloudClipboard` = 0) so transcripts never land in clipboard history or
//!    the cloud clipboard.
//! 3. Synthesize Ctrl+V with `SendInput` using **virtual key 0x56** — not
//!    `KEYEVENTF_UNICODE 'v'`, which types a literal "v" instead of pasting.
//! 4. After a short delay, restore the snapshot **only if** `GetClipboardSequenceNumber`
//!    still matches what we set (pure decision: `crate::inject::should_restore`).

use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    SendInput, VIRTUAL_KEY, VK_CONTROL, VK_RETURN, VK_V,
};
use windows::core::w;

use crate::inject::{TextInjector, normalize_crlf, should_restore, utf16_nul};
use crate::{Error, Result};

const CLIPBOARD_OPEN_ATTEMPTS: usize = 8;
const CLIPBOARD_OPEN_RETRY: Duration = Duration::from_millis(5);
/// How long focused apps get to service the paste before we restore the clipboard.
const DEFAULT_RESTORE_DELAY: Duration = Duration::from_millis(300);
/// SendInput batch size for unicode typing (units, i.e. half of the INPUT count).
const TYPE_CHUNK_UNITS: usize = 256;

/// Clipboard-paste text injector for Windows.
///
/// TODO(seam): the clipboard snapshot is `CF_UNICODETEXT`-only. Rich content (images,
/// files, HTML) on the user's clipboard is not restored after a paste — restoring it needs
/// an `OleGetClipboard`/`OleSetClipboard` snapshot (see OpenWritr's `paste.rs`).
///
/// TODO(seam): if the user is still physically holding hotkey modifiers when `inject_into`
/// runs, the synthesized Ctrl+V combines with them (e.g. Ctrl+Win+V). The app layer should
/// inject only after the hotkey `Released` event, as OwlWhisp's worker does.
pub struct WindowsInjector {
    restore_delay: Duration,
}

impl WindowsInjector {
    /// Injector with the default 300 ms clipboard-restore delay.
    pub fn new() -> Self {
        Self {
            restore_delay: DEFAULT_RESTORE_DELAY,
        }
    }

    /// Override how long to wait before restoring the previous clipboard.
    pub fn with_restore_delay(mut self, delay: Duration) -> Self {
        self.restore_delay = delay;
        self
    }
}

impl Default for WindowsInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl TextInjector for WindowsInjector {
    fn inject_into(&mut self, text: &str, restore_clipboard: bool) -> Result<()> {
        // 1. Snapshot the current clipboard text (and the sequence so we can detect races).
        let (previous_text, sequence_at_snapshot) = {
            let guard = ClipboardGuard::open()?;
            let text = read_clipboard_text();
            drop(guard);
            // SAFETY: no preconditions; returns the global clipboard change counter.
            (text, unsafe { GetClipboardSequenceNumber() })
        };

        // 2. Stage our text with the history/cloud exclusion formats.
        let staged = normalize_crlf(text);
        {
            let guard = ClipboardGuard::open()?;
            // SAFETY: clipboard is open (guard); checking the counter is side-effect free.
            if unsafe { GetClipboardSequenceNumber() } != sequence_at_snapshot {
                return Err(Error::Inject(
                    "clipboard changed while preparing the paste; aborted".into(),
                ));
            }
            write_clipboard_text_excluded(&staged)?;
            guard.close()?;
        }
        // SAFETY: no preconditions.
        let sequence_after_set = unsafe { GetClipboardSequenceNumber() };

        // 3. Synthesize Ctrl+V (virtual keys, marked so our own hook ignores them).
        send_ctrl_v()?;

        // 4. Guarded restore.
        if restore_clipboard {
            std::thread::sleep(self.restore_delay);
            // SAFETY: no preconditions.
            let sequence_now = unsafe { GetClipboardSequenceNumber() };
            if should_restore(sequence_after_set, sequence_now) {
                let guard = ClipboardGuard::open()?;
                // Re-check under the open clipboard: opening can race a writer.
                // SAFETY: clipboard is open (guard).
                if unsafe { GetClipboardSequenceNumber() } == sequence_after_set {
                    match &previous_text {
                        Some(prev) => write_clipboard_text_plain(prev)?,
                        None => {
                            // Previous clipboard had no text; leave it empty.
                            // SAFETY: clipboard is open (guard).
                            unsafe { EmptyClipboard() }
                                .map_err(|e| Error::Inject(format!("EmptyClipboard: {e}")))?;
                        }
                    }
                }
                guard.close()?;
            } else {
                tracing::debug!("clipboard changed externally after paste; keeping newer contents");
            }
        }
        Ok(())
    }

    fn type_unicode(&mut self, text: &str) -> Result<()> {
        // Normalize newlines to a single \n and type those as VK_RETURN so multiline text
        // behaves in editors and chat inputs.
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut inputs: Vec<INPUT> = Vec::with_capacity(TYPE_CHUNK_UNITS * 2);
        for ch in normalized.chars() {
            if ch == '\n' {
                inputs.push(key_input(VK_RETURN, KEYBD_EVENT_FLAGS(0)));
                inputs.push(key_input(VK_RETURN, KEYEVENTF_KEYUP));
            } else {
                let mut units = [0u16; 2];
                for &unit in ch.encode_utf16(&mut units).iter() {
                    inputs.push(unicode_input(unit, KEYEVENTF_UNICODE));
                    inputs.push(unicode_input(unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                }
            }
            if inputs.len() >= TYPE_CHUNK_UNITS * 2 {
                send_inputs(&inputs)?;
                inputs.clear();
            }
        }
        if !inputs.is_empty() {
            send_inputs(&inputs)?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for WindowsInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsInjector")
            .field("restore_delay", &self.restore_delay)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// SendInput helpers
// ---------------------------------------------------------------------------

fn key_input(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: super::LW_SENDINPUT_MARKER,
            },
        },
    }
}

fn unicode_input(unit: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: super::LW_SENDINPUT_MARKER,
            },
        },
    }
}

fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    // SAFETY: inputs is a valid slice of fully initialized INPUT structs; cbsize is the
    // documented size of one INPUT.
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(Error::Inject(format!(
            "SendInput injected {sent}/{} events: {}",
            inputs.len(),
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn send_ctrl_v() -> Result<()> {
    let inputs = [
        key_input(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_V, KEYBD_EVENT_FLAGS(0)),
        key_input(VK_V, KEYEVENTF_KEYUP),
        key_input(VK_CONTROL, KEYEVENTF_KEYUP),
    ];
    send_inputs(&inputs)
}

// ---------------------------------------------------------------------------
// Clipboard plumbing
// ---------------------------------------------------------------------------

/// RAII open/close for the Win32 clipboard, with retries (the clipboard is a single
/// global mutex other apps briefly hold).
struct ClipboardGuard {
    closed: bool,
}

impl ClipboardGuard {
    fn open() -> Result<Self> {
        let mut attempts = CLIPBOARD_OPEN_ATTEMPTS;
        loop {
            // SAFETY: opening with no owner window is allowed; paired with CloseClipboard.
            match unsafe { OpenClipboard(None) } {
                Ok(()) => return Ok(Self { closed: false }),
                Err(e) => {
                    if attempts == 0 {
                        return Err(Error::Inject(format!("OpenClipboard: {e}")));
                    }
                    attempts -= 1;
                    std::thread::sleep(CLIPBOARD_OPEN_RETRY);
                }
            }
        }
    }

    fn close(mut self) -> Result<()> {
        self.closed = true;
        // SAFETY: we own the open clipboard.
        unsafe { CloseClipboard() }.map_err(|e| Error::Inject(format!("CloseClipboard: {e}")))
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        if !self.closed {
            // SAFETY: we own the open clipboard; best-effort close on unwind.
            let _ = unsafe { CloseClipboard() };
        }
    }
}

/// Read `CF_UNICODETEXT` from the (already open) clipboard.
fn read_clipboard_text() -> Option<String> {
    // SAFETY: clipboard is open (caller holds ClipboardGuard); probing a format is safe.
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32) }.is_err() {
        return None;
    }
    // SAFETY: clipboard is open; the returned handle is owned by the clipboard and valid
    // while it stays open — we copy the data out before closing.
    let handle = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }.ok()?;
    if handle.is_invalid() {
        return None;
    }
    let hglobal = HGLOBAL(handle.0);
    // SAFETY: handle is a global-memory clipboard handle; GlobalSize/GlobalLock are the
    // documented way to access it. We unlock before returning.
    unsafe {
        let byte_len = GlobalSize(hglobal);
        if byte_len < 2 {
            return None;
        }
        let ptr = GlobalLock(hglobal).cast::<u16>();
        if ptr.is_null() {
            return None;
        }
        let units = std::slice::from_raw_parts(ptr, byte_len / 2);
        let text_len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..text_len]);
        // GlobalUnlock returns Err(NO_ERROR-like) when the count hits zero; ignore.
        let _ = GlobalUnlock(hglobal);
        Some(text)
    }
}

/// Move `bytes` into a `GMEM_MOVEABLE` allocation suitable for `SetClipboardData`.
fn hglobal_from_bytes(bytes: &[u8]) -> Result<HGLOBAL> {
    // SAFETY: allocating movable global memory of the exact payload size.
    let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) }
        .map_err(|e| Error::Inject(format!("GlobalAlloc: {e}")))?;
    // SAFETY: hglobal was just allocated with at least bytes.len() bytes; we lock, copy,
    // and unlock before anyone else can see the handle.
    unsafe {
        let dst = GlobalLock(hglobal).cast::<u8>();
        if dst.is_null() {
            let _ = windows::Win32::Foundation::GlobalFree(Some(hglobal));
            return Err(Error::Inject("GlobalLock failed for clipboard payload".into()));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        let _ = GlobalUnlock(hglobal);
    }
    Ok(hglobal)
}

/// `SetClipboardData` with ownership transfer (frees the allocation on failure).
fn set_clipboard_bytes(format: u32, bytes: &[u8]) -> Result<()> {
    let hglobal = hglobal_from_bytes(bytes)?;
    // SAFETY: clipboard is open (caller holds ClipboardGuard); on success the system owns
    // the memory, on failure we free it.
    match unsafe { SetClipboardData(format, Some(HANDLE(hglobal.0))) } {
        Ok(_) => Ok(()),
        Err(e) => {
            // SAFETY: SetClipboardData failed, so ownership stayed with us.
            let _ = unsafe { windows::Win32::Foundation::GlobalFree(Some(hglobal)) };
            Err(Error::Inject(format!("SetClipboardData({format}): {e}")))
        }
    }
}

/// Write plain `CF_UNICODETEXT` (used for restoring the user's previous text).
/// Caller must hold the clipboard open.
fn write_clipboard_text_plain(text: &str) -> Result<()> {
    // SAFETY: clipboard is open (caller holds ClipboardGuard).
    unsafe { EmptyClipboard() }.map_err(|e| Error::Inject(format!("EmptyClipboard: {e}")))?;
    let units = utf16_nul(text);
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
    set_clipboard_bytes(CF_UNICODETEXT.0 as u32, &bytes)
}

/// Write our transcript with the Windows 11 history/cloud exclusion formats.
/// Caller must hold the clipboard open.
fn write_clipboard_text_excluded(text: &str) -> Result<()> {
    write_clipboard_text_plain(text)?;
    for (name, format) in [
        (
            "ExcludeClipboardContentFromMonitorProcessing",
            exclusion_format_monitor(),
        ),
        ("CanIncludeInClipboardHistory", exclusion_format_history()),
        ("CanUploadToCloudClipboard", exclusion_format_cloud()),
    ] {
        match format {
            0 => tracing::debug!(name, "clipboard exclusion format failed to register"),
            f => {
                // DWORD 0: "do not include in history / do not upload". For the monitor
                // format the mere presence is what matters.
                set_clipboard_bytes(f, &0u32.to_le_bytes())?;
            }
        }
    }
    Ok(())
}

fn exclusion_format_monitor() -> u32 {
    // SAFETY: w! produces a valid NUL-terminated wide string.
    unsafe { RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")) }
}
fn exclusion_format_history() -> u32 {
    // SAFETY: as above.
    unsafe { RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")) }
}
fn exclusion_format_cloud() -> u32 {
    // SAFETY: as above.
    unsafe { RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusion_formats_register() {
        // RegisterClipboardFormatW does not need the clipboard to be open.
        assert_ne!(exclusion_format_monitor(), 0);
        assert_ne!(exclusion_format_history(), 0);
        assert_ne!(exclusion_format_cloud(), 0);
        // Registering the same name twice yields the same atom.
        assert_eq!(exclusion_format_history(), exclusion_format_history());
    }

    #[test]
    fn ctrl_v_uses_virtual_keys_not_unicode() {
        let inputs = [
            key_input(VK_CONTROL, KEYBD_EVENT_FLAGS(0)),
            key_input(VK_V, KEYBD_EVENT_FLAGS(0)),
            key_input(VK_V, KEYEVENTF_KEYUP),
            key_input(VK_CONTROL, KEYEVENTF_KEYUP),
        ];
        // SAFETY (test): reading the union field we just initialized.
        unsafe {
            assert_eq!(inputs[1].Anonymous.ki.wVk, VK_V); // 0x56, not a unicode scan
            assert_eq!(inputs[1].Anonymous.ki.wScan, 0);
            assert_eq!(
                inputs[1].Anonymous.ki.dwFlags & KEYEVENTF_UNICODE,
                KEYBD_EVENT_FLAGS(0)
            );
            assert_eq!(inputs[2].Anonymous.ki.dwFlags & KEYEVENTF_KEYUP, KEYEVENTF_KEYUP);
            for i in &inputs {
                assert_eq!(i.Anonymous.ki.dwExtraInfo, crate::windows::LW_SENDINPUT_MARKER);
            }
        }
    }

    #[test]
    fn unicode_input_carries_the_unit_in_wscan() {
        let i = unicode_input(0xD834, KEYEVENTF_UNICODE);
        // SAFETY (test): reading the union field we just initialized.
        unsafe {
            assert_eq!(i.Anonymous.ki.wScan, 0xD834);
            assert_eq!(i.Anonymous.ki.wVk, VIRTUAL_KEY(0));
            assert_eq!(i.Anonymous.ki.dwFlags, KEYEVENTF_UNICODE);
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Put text into whatever window is in front, by the exact path dictation uses.
    ///
    /// Ignored by default: it takes the clipboard and synthesizes a paste into somebody's window.
    /// Run it with the application focused on the Dictate tab, caret in the scratchpad, to see
    /// whether a transcript would actually land there:
    /// `cargo test -p lw-platform --lib inject::live_tests -- --ignored --nocapture`
    #[test]
    #[ignore = "takes the clipboard and types into the focused window"]
    fn inject_into_the_focused_window() {
        let mut injector = WindowsInjector::new();
        injector.inject_into("LW-INJECT-PROBE", true).expect("inject");
        println!("injected LW-INJECT-PROBE into the foreground window");
    }
}
