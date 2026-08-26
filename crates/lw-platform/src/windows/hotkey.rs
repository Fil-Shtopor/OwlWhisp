//! Global hotkeys on Windows via a `WH_KEYBOARD_LL` low-level keyboard hook.
//!
//! Why a hook and not `RegisterHotKey`/`GetAsyncKeyState`:
//! - `RegisterHotKey` only fires on key-*down*; push-to-talk needs the release edge.
//! - `GetAsyncKeyState` lies during focus changes (Windows synthesizes key-ups for the new
//!   foreground window), which aborts recordings mid-hold.
//!
//! The hook callback must be fast (< the ~300 ms `LowLevelHooksTimeout` budget, or Windows
//! silently unhooks us) and is therefore lock-free: the combo configuration lives in
//! atomics, the edge detection runs in a `thread_local` [`ComboTracker`] on the hook
//! thread (LL-hook callbacks are serialized on the installing thread), and events go out
//! through a pre-allocated bounded crossbeam channel via `try_send`.
//!
//! The hook itself is installed once on a dedicated message-pump thread and stays for the
//! lifetime of the process (dropping `WindowsHotkey` clears the combo, which makes the
//! callback a no-op pass-through). Modeled on OpenWritr's `key_hook.rs`.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, KBDLLHOOKSTRUCT, MSG, PM_REMOVE, PeekMessageW, SetWindowsHookExW,
    TranslateMessage, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::hotkey::{ComboTracker, GlobalHotkey, HotkeyEvent, HotkeySpec, ParsedCombo, parse_spec};
use crate::{Error, Result};

/// Active combo configuration, readable lock-free from the hook callback.
/// `GEN` is bumped after every change; the callback rebuilds its tracker when it observes
/// a new generation.
static COMBO_MODS: AtomicU32 = AtomicU32::new(0);
static COMBO_TRIGGER: AtomicU32 = AtomicU32::new(0);
static COMBO_GEN: AtomicU32 = AtomicU32::new(0);
/// Mirror of the tracker's held-state so `unregister` can synthesize a final `Released`.
static COMBO_ACTIVE: AtomicBool = AtomicBool::new(false);

static HOOK_STARTED: AtomicBool = AtomicBool::new(false);
static HOOK_HEALTHY: AtomicBool = AtomicBool::new(false);

/// The event channel outlives everything (the hook callback holds no owned sender).
static EVENTS: OnceLock<(Sender<HotkeyEvent>, Receiver<HotkeyEvent>)> = OnceLock::new();

fn events_channel() -> &'static (Sender<HotkeyEvent>, Receiver<HotkeyEvent>) {
    EVENTS.get_or_init(|| bounded(128))
}

fn load_combo() -> ParsedCombo {
    ParsedCombo {
        mods: COMBO_MODS.load(Ordering::Acquire) as u8,
        trigger: COMBO_TRIGGER.load(Ordering::Acquire) as u16,
    }
}

thread_local! {
    /// (combo generation, tracker) — lives on the hook thread only.
    static TRACKER: RefCell<(u32, ComboTracker)> =
        RefCell::new((u32::MAX, ComboTracker::new(ParsedCombo::default())));
}

/// The low-level keyboard hook callback. Runs on the hook thread for every physical key
/// event in the system. Must stay allocation-free and fast.
unsafe extern "system" fn ll_kbd_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Per the WH_KEYBOARD_LL contract: code < 0 must be passed straight through.
    if code >= 0 {
        // SAFETY: for WH_KEYBOARD_LL with code >= 0, lparam points to a valid
        // KBDLLHOOKSTRUCT for the duration of the call (Win32 contract).
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        // Ignore key events we injected ourselves (Ctrl+V paste, unicode typing).
        if kb.dwExtraInfo != super::LW_SENDINPUT_MARKER {
            let msg = wparam.0 as u32;
            let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
            if down || up {
                on_physical_key(kb.vkCode, down);
            }
        }
    }
    // SAFETY: forwarding the exact arguments we received, as the hook contract requires.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Edge detection for one physical key transition (hook thread only).
fn on_physical_key(vk_code: u32, down: bool) {
    TRACKER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let generation = COMBO_GEN.load(Ordering::Acquire);
        if slot.0 != generation {
            *slot = (generation, ComboTracker::new(load_combo()));
        }
        if slot.1.combo().is_empty() {
            return;
        }
        if let Some(event) = slot.1.on_key(vk_code, down) {
            COMBO_ACTIVE.store(event == HotkeyEvent::Pressed, Ordering::Release);
            // Pre-allocated bounded channel: try_send never allocates; drop on overflow
            // rather than ever blocking the hook.
            let _ = events_channel().0.try_send(event);
        }
    });
}

/// Install the hook on its dedicated message-pump thread (idempotent). Blocks briefly
/// until the hook reports success or failure.
fn ensure_hook() -> Result<()> {
    events_channel();
    if HOOK_STARTED.swap(true, Ordering::SeqCst) {
        if HOOK_HEALTHY.load(Ordering::SeqCst) {
            return Ok(());
        }
        return Err(Error::Hotkey(
            "keyboard hook thread previously failed to install".into(),
        ));
    }

    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<std::result::Result<(), String>>(1);
    std::thread::Builder::new()
        .name("lw-hotkey-hook".into())
        .spawn(move || hook_thread(ready_tx))
        .map_err(|e| {
            HOOK_STARTED.store(false, Ordering::SeqCst);
            Error::Hotkey(format!("spawn hook thread: {e}"))
        })?;

    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => {
            HOOK_HEALTHY.store(true, Ordering::SeqCst);
            Ok(())
        }
        Ok(Err(msg)) => {
            HOOK_STARTED.store(false, Ordering::SeqCst);
            Err(Error::Hotkey(msg))
        }
        Err(_) => Err(Error::Hotkey("keyboard hook install timed out".into())),
    }
}

/// Dedicated hook thread: installs the LL hook and pumps messages forever.
/// The hook is intentionally leaked — it lives for the rest of the process.
fn hook_thread(ready_tx: std::sync::mpsc::SyncSender<std::result::Result<(), String>>) {
    // SAFETY: installing a global WH_KEYBOARD_LL hook with a valid callback; hmod may be
    // null for LL hooks (the callback lives in this module, not a DLL).
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_kbd_proc), None, 0) };
    let _hook = match hook {
        Ok(h) => h,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("SetWindowsHookExW(WH_KEYBOARD_LL) failed: {e}")));
            return;
        }
    };
    tracing::info!("low-level keyboard hook installed");
    let _ = ready_tx.send(Ok(()));

    // LL-hook callbacks are delivered while this thread pumps messages. PeekMessage plus a
    // short sleep keeps latency low without burning a core.
    let mut msg = MSG::default();
    loop {
        // SAFETY: msg is a valid, owned MSG; standard message pump.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Global hotkey detection backed by the process-wide low-level keyboard hook.
///
/// Only one combo is active at a time process-wide (LocalWisper needs exactly one
/// push-to-talk combo); creating a second `WindowsHotkey` shares the same hook and
/// configuration.
pub struct WindowsHotkey {
    registered: bool,
}

impl WindowsHotkey {
    /// Install (or attach to) the process-wide keyboard hook.
    pub fn new() -> Result<Self> {
        ensure_hook()?;
        Ok(Self { registered: false })
    }

    fn publish_combo(combo: ParsedCombo) {
        COMBO_MODS.store(combo.mods as u32, Ordering::Release);
        COMBO_TRIGGER.store(combo.trigger as u32, Ordering::Release);
        COMBO_GEN.fetch_add(1, Ordering::Release);
        // The hook thread rebuilds its tracker (all-keys-up) on the next event; if the old
        // combo was mid-hold, deliver the closing edge ourselves.
        if COMBO_ACTIVE.swap(false, Ordering::AcqRel) {
            let _ = events_channel().0.try_send(HotkeyEvent::Released);
        }
    }
}

impl GlobalHotkey for WindowsHotkey {
    fn register(&mut self, spec: &HotkeySpec) -> Result<()> {
        let combo = parse_spec(spec)?;
        Self::publish_combo(combo);
        self.registered = true;
        tracing::info!(?spec, "hotkey registered");
        Ok(())
    }

    fn unregister(&mut self) -> Result<()> {
        if self.registered {
            Self::publish_combo(ParsedCombo::default());
            self.registered = false;
            tracing::info!("hotkey unregistered");
        }
        Ok(())
    }

    fn events(&self) -> Receiver<HotkeyEvent> {
        events_channel().1.clone()
    }
}

impl Drop for WindowsHotkey {
    fn drop(&mut self) {
        let _ = self.unregister();
    }
}

impl std::fmt::Debug for WindowsHotkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsHotkey")
            .field("registered", &self.registered)
            .finish()
    }
}
