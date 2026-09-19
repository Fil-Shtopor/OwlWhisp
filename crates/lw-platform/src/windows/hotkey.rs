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
    CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, MSG, SetTimer,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
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

/// Milliseconds (since [`epoch`]) at which the hook last saw any key event at all.
///
/// Evidence that the hook is still installed. Windows removes a low-level hook silently when its
/// thread fails to answer within `LowLevelHooksTimeout`, and offers no way to ask whether that has
/// happened; a key event arriving is the only proof there is.
static LAST_EVENT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How long the hook may go without seeing a single key before it is re-installed.
///
/// Long enough that an ordinary pause in typing does not cause churn, short enough that a user who
/// comes back to the keyboard finds a working hotkey.
const WATCHDOG_MS: u32 = 5_000;

/// Process start, so elapsed time fits in an atomic.
fn epoch() -> std::time::Instant {
    static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
    *EPOCH.get_or_init(std::time::Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64
}

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
                LAST_EVENT_MS.store(now_ms(), Ordering::Relaxed);
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
    let mut hook = match install_hook() {
        Ok(h) => h,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };
    tracing::info!("low-level keyboard hook installed");
    LAST_EVENT_MS.store(now_ms(), Ordering::Relaxed);
    let _ = ready_tx.send(Ok(()));

    // A timer, so the loop can wake for the watchdog without ever leaving the message call.
    // SAFETY: a thread timer with no window and no callback; its WM_TIMER arrives below.
    let _ = unsafe { SetTimer(None, 0, WATCHDOG_MS, None) };

    // `GetMessageW`, not `PeekMessage` and a sleep.
    //
    // This is the whole reason the hotkey used to stop working at random. The system delivers a
    // low-level hook callback only while the installing thread is *inside* a message-retrieval
    // call, and it gives that thread `LowLevelHooksTimeout` (300 ms by default) to answer before
    // it silently removes the hook -- no error, no event, no way to ask afterwards. A loop that
    // peeks and then sleeps spends nearly all of its time outside a message call, and on a busy
    // machine -- loading an execution provider, running a benchmark -- a 5 ms sleep becomes long
    // enough to miss the window. The hook would be dropped moments after startup and the
    // application would go on believing it was registered.
    //
    // `GetMessageW` parks the thread inside the call, which is exactly where it has to be.
    let mut msg = MSG::default();
    // SAFETY: msg is a valid, owned MSG; standard blocking message pump. A return of -1 is an
    // error and 0 is WM_QUIT; both end the loop.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        if msg.message == WM_TIMER {
            hook = watchdog(hook);
            continue;
        }
        // SAFETY: msg came from GetMessageW and is valid for these calls.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Install the hook, returning its handle.
fn install_hook() -> std::result::Result<HHOOK, String> {
    // SAFETY: installing a global WH_KEYBOARD_LL hook with a valid callback; hmod may be
    // null for LL hooks (the callback lives in this module, not a DLL).
    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_kbd_proc), None, 0) }
        .map_err(|e| format!("SetWindowsHookExW(WH_KEYBOARD_LL) failed: {e}"))
}

/// Re-install the hook if nothing has been seen through it for a while.
///
/// There is no API that answers "is my hook still installed", so silence is the only signal
/// available -- and silence is ambiguous: it also means nobody is typing. Re-installing in that
/// case costs two calls and nothing else, which is a good trade against a hotkey that is dead for
/// the rest of the session.
fn watchdog(current: HHOOK) -> HHOOK {
    let quiet = now_ms().saturating_sub(LAST_EVENT_MS.load(Ordering::Relaxed));
    if quiet < u64::from(WATCHDOG_MS) {
        return current;
    }
    // SAFETY: `current` came from SetWindowsHookExW on this thread and is unhooked once.
    unsafe {
        let _ = UnhookWindowsHookEx(current);
    }
    match install_hook() {
        Ok(h) => {
            tracing::debug!("keyboard hook re-installed after {quiet} ms of silence");
            LAST_EVENT_MS.store(now_ms(), Ordering::Relaxed);
            HOOK_HEALTHY.store(true, Ordering::SeqCst);
            h
        }
        Err(e) => {
            // Now genuinely broken, and the status the front end reads must say so rather than
            // keep claiming a registration that no longer exists.
            tracing::error!("the keyboard hook could not be re-installed: {e}");
            HOOK_HEALTHY.store(false, Ordering::SeqCst);
            current
        }
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

#[cfg(test)]
mod live_tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
    };

    /// Synthesize one key event the hook will actually see.
    ///
    /// Deliberately *without* [`super::super::LW_SENDINPUT_MARKER`]: the hook drops events
    /// carrying it, which is right for our own paste keystrokes and would make this test pass
    /// while proving nothing.
    fn key(vk: u16, up: bool) {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    }

    /// The whole hook, for real: install it, hold a combo down, let go, and see both edges.
    ///
    /// Ignored by default. It installs a system-wide keyboard hook and injects keystrokes into
    /// whatever has focus, which is not something a test run should do behind someone's back --
    /// and on a headless CI box there is no input queue to inject into. Run it deliberately:
    /// `cargo test -p lw-platform --lib live_tests -- --ignored --test-threads=1`.
    ///
    /// Ctrl+Alt+F13 on purpose: F13 is absent from ordinary keyboards, so nothing else on the
    /// machine is listening for it and the injected keys cannot trigger anything.
    /// Inject `mods + trigger` once and report whether the hook saw both edges.
    fn probe(mods: &[&str], trigger: &str) -> bool {
        let mut hotkey = match WindowsHotkey::new() {
            Ok(h) => h,
            Err(_) => return false,
        };
        let events = hotkey.events();
        if hotkey
            .register(&HotkeySpec {
                modifiers: mods.iter().map(|s| s.to_string()).collect(),
                trigger: trigger.into(),
            })
            .is_err()
        {
            return false;
        }
        while events.try_recv().is_ok() {}

        let mut codes: Vec<u16> = mods
            .iter()
            .map(|m| match *m {
                "ctrl" => 0x11u16,
                "shift" => 0x10,
                "alt" => 0x12,
                _ => 0x5B,
            })
            .collect();
        codes.push(crate::hotkey::trigger_vk(trigger).unwrap_or(0));

        for c in &codes {
            key(*c, false);
        }
        let pressed = events.recv_timeout(Duration::from_millis(1500));
        for c in codes.iter().rev() {
            key(*c, true);
        }
        let released = events.recv_timeout(Duration::from_millis(1500));
        pressed == Ok(HotkeyEvent::Pressed) && released == Ok(HotkeyEvent::Released)
    }

    /// Which combinations this machine actually delivers to a low-level hook.
    ///
    /// Not a pass/fail of our code -- every one of these is registered and tracked identically, and
    /// the pure edge detector is covered by ordinary tests. This answers a different question, and
    /// the only way to answer it is on the machine: *another* hook, an IME or a utility can take a
    /// combination before ours sees it, and the symptom is a hotkey that silently does nothing.
    ///
    /// Run it with `--nocapture` when a binding "does not work" and nothing is in the log.
    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    fn report_which_combinations_this_machine_delivers() {
        for (mods, trigger) in [
            (&["ctrl"][..], "space"),
            (&["ctrl", "alt"][..], "space"),
            (&["ctrl", "shift"][..], "space"),
            (&["alt"][..], "space"),
            (&["ctrl"][..], "d"),
            (&["ctrl", "alt"][..], "f13"),
        ] {
            let ok = probe(mods, trigger);
            println!(
                "{:<24} {}",
                format!("{}+{trigger}", mods.join("+")),
                if ok { "delivered" } else { "TAKEN BY SOMETHING ELSE" }
            );
        }
    }

    /// The same, for the binding this machine is actually configured with.
    ///
    /// Ctrl+Space is not an arbitrary second case: it is a combination Windows itself uses for the
    /// IME language switch, so it is the one most likely to be eaten by a hook ahead of ours.
    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    fn ctrl_space_produces_both_edges() {
        const VK_CONTROL: u16 = 0x11;
        const VK_SPACE: u16 = 0x20;

        let mut hotkey = WindowsHotkey::new().expect("install the keyboard hook");
        let events = hotkey.events();
        hotkey
            .register(&HotkeySpec {
                modifiers: vec!["ctrl".into()],
                trigger: "space".into(),
            })
            .expect("register ctrl+space");
        while events.try_recv().is_ok() {}

        key(VK_CONTROL, false);
        key(VK_SPACE, false);
        let pressed = events.recv_timeout(Duration::from_secs(2));
        key(VK_SPACE, true);
        key(VK_CONTROL, true);
        let released = events.recv_timeout(Duration::from_secs(2));

        assert_eq!(pressed, Ok(HotkeyEvent::Pressed), "press edge");
        assert_eq!(released, Ok(HotkeyEvent::Released), "release edge");
    }

    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    fn a_held_combo_produces_a_press_edge_and_letting_go_produces_a_release_edge() {
        const VK_CONTROL: u16 = 0x11;
        const VK_MENU: u16 = 0x12; // Alt
        const VK_F13: u16 = 0x7C;

        let mut hotkey = WindowsHotkey::new().expect("install the keyboard hook");
        let events = hotkey.events();
        hotkey
            .register(&HotkeySpec {
                modifiers: vec!["ctrl".into(), "alt".into()],
                trigger: "f13".into(),
            })
            .expect("register ctrl+alt+f13");

        // Drain anything the machine was already holding when the hook went in.
        while events.try_recv().is_ok() {}

        key(VK_CONTROL, false);
        key(VK_MENU, false);
        key(VK_F13, false);
        let pressed = events.recv_timeout(Duration::from_secs(2));

        key(VK_F13, true);
        key(VK_MENU, true);
        key(VK_CONTROL, true);
        let released = events.recv_timeout(Duration::from_secs(2));

        assert_eq!(pressed, Ok(HotkeyEvent::Pressed), "press edge");
        assert_eq!(released, Ok(HotkeyEvent::Released), "release edge");
    }
}
