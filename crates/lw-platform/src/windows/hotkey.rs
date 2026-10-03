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

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
// Aliased: this crate has its own `MOD_*` family bits, and the two sets mean different things.
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT as WIN_MOD_ALT, MOD_CONTROL as WIN_MOD_CONTROL,
    MOD_NOREPEAT as WIN_MOD_NOREPEAT, MOD_SHIFT as WIN_MOD_SHIFT, MOD_WIN as WIN_MOD_WIN,
    RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, MSG, PostThreadMessageW,
    PeekMessageW, PM_NOREMOVE, SetTimer, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WM_APP,
    WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
};

use crate::hotkey::{
    CaptureTracker, Captured, ComboTracker, GlobalHotkey, HotkeyCapture, HotkeyEvent, HotkeySpec,
    MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN, ParsedCombo, SwallowLedger, parse_spec, swallow_trigger,
};
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

/// Every key transition the hook callback has been handed, and every one the grab has read.
///
/// Counters rather than log lines because this is counted inside a low-level hook callback, which
/// must not allocate and must not block. They exist to tell two very different faults apart: a
/// grab that read nothing because the keys had no name, and a grab that read nothing because the
/// hook was not on the keyboard at all. Without them the two look identical from the outside.
static HOOK_KEYS_SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static GRAB_KEYS_SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many key transitions the hook has seen in this process, and how many of those the shortcut
/// editor's grab was handed.
pub fn key_counts() -> (u64, u64) {
    (
        HOOK_KEYS_SEEN.load(Ordering::Relaxed),
        GRAB_KEYS_SEEN.load(Ordering::Relaxed),
    )
}

/// The hook thread, so a rebind can be handed to it -- `RegisterHotKey` delivers `WM_HOTKEY` to the
/// thread that called it, so it has to be the thread that is pumping messages.
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);

/// Whether Windows itself is delivering the combination, rather than the hook recognising it.
///
/// The preferred path, and the one the build before this used. `RegisterHotKey` is arranged by the
/// window manager: it consumes the key, it cannot be starved by a slow callback, and it does not
/// depend on this process winning a race in a chain of hooks that other applications also install
/// into. The hook stays for what `RegisterHotKey` cannot do -- the release edge that push-to-talk
/// needs, and modifier-only combinations, which it refuses outright.
static USE_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Our one hotkey registration.
const HOTKEY_ID: i32 = 0x4C57; // "LW"

/// Ask the hook thread to re-register with the window manager.
const WM_REBIND: u32 = WM_APP + 1;

/// Bumped by the hook thread every time it finishes putting the hook back.
///
/// The counter is how a caller on another thread can *wait* for that, and waiting is the point:
/// `PostThreadMessageW` returns immediately, so without this the grab would arm while the hook was
/// still down, and the keys pressed in that gap would be seen by nobody. It is a gap of
/// microseconds, but a shortcut editor that occasionally reads Ctrl+F13 as plain F13 is worse than
/// one that takes a moment to open.
static REHOOK_GEN: AtomicU32 = AtomicU32::new(0);

/// Ask the hook thread to put the low-level hook back, whether or not it looks alive.
///
/// The shortcut editor is the one caller that cannot tolerate a hook that has quietly gone. While
/// the window manager holds the hotkey the hook is on nobody's path, so no key event is expected
/// through it and the watchdog has nothing to notice a removal *by* -- the binding goes on working
/// regardless, which is exactly why the removal stays invisible until something else needs it.
const WM_REHOOK: u32 = WM_APP + 2;

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

/// Post one of our own messages to the hook thread.
///
/// Both of them have to run there and nowhere else: `RegisterHotKey` delivers `WM_HOTKEY` to
/// whichever thread called it, and a low-level hook delivers its callbacks to whichever thread
/// installed it.
fn post_to_hook_thread(message: u32) {
    let thread = HOOK_THREAD.load(Ordering::Acquire);
    if thread == 0 {
        return;
    }
    // SAFETY: a thread message to our own hook thread; failure only means it has gone.
    unsafe {
        let _ = PostThreadMessageW(thread, message, WPARAM(0), LPARAM(0));
    }
}

/// The event channel outlives everything (the hook callback holds no owned sender).
static EVENTS: OnceLock<(Sender<HotkeyEvent>, Receiver<HotkeyEvent>)> = OnceLock::new();
static REBIND_REQUEST: AtomicU32 = AtomicU32::new(0);
static REBIND_ACK: OnceLock<(
    Sender<(u32, std::result::Result<(), String>)>,
    Receiver<(u32, std::result::Result<(), String>)>,
)> = OnceLock::new();

fn events_channel() -> &'static (Sender<HotkeyEvent>, Receiver<HotkeyEvent>) {
    EVENTS.get_or_init(|| bounded(128))
}

fn rebind_ack() -> &'static (
    Sender<(u32, std::result::Result<(), String>)>,
    Receiver<(u32, std::result::Result<(), String>)>,
) {
    REBIND_ACK.get_or_init(unbounded)
}

fn rebind_and_wait() -> Result<()> {
    let ack = rebind_ack();
    let request = REBIND_REQUEST.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    let thread = HOOK_THREAD.load(Ordering::Acquire);
    if thread == 0 {
        return Err(Error::Hotkey("keyboard hook thread is unavailable".into()));
    }
    // SAFETY: the hook thread creates its message queue before announcing readiness.
    unsafe { PostThreadMessageW(thread, WM_REBIND, WPARAM(request as usize), LPARAM(0)) }
        .map_err(|e| Error::Hotkey(format!("request hotkey registration: {e}")))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let (answered, result) = ack
            .1
            .recv_timeout(remaining)
            .map_err(|e| Error::Hotkey(format!("hotkey registration did not finish: {e}")))?;
        if answered == request {
            return result.map_err(Error::Hotkey);
        }
    }
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
    /// Whether the trigger's key-down was taken, so its key-up is taken as well.
    static SWALLOWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
                if HOOK_KEYS_SEEN.fetch_add(1, Ordering::Relaxed) == 0 {
                    // Once per process, so the log can answer "did the hook ever fire at all",
                    // which is the first question whenever a shortcut does nothing.
                    // SAFETY: no preconditions; the id of the thread the callback runs on.
                    let thread =
                        unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
                    tracing::info!(callback_thread = thread, "keyboard hook is receiving keys");
                }
                // The shortcut editor's grab comes first and is exclusive: while it is on, the
                // combo tracker must not see the keys either, or pressing the *current* hotkey to
                // look at it would start a dictation behind the settings window.
                let capturing = CAPTURE_STATE.load(Ordering::Acquire) != CAPTURE_OFF;
                if capturing {
                    // Fed the event before anything is decided about taking it: the release that
                    // ends a modifiers-only gesture is also a release this process owes, and it
                    // has to be *read* before it is taken.
                    let take = on_capture_key(kb.vkCode, down);
                    if down && take {
                        SWALLOWED.with(|l| l.borrow_mut().took_press(kb.vkCode));
                        return LRESULT(1);
                    }
                }
                // A release whose press this process took is taken as well -- whether or not the
                // grab is still open, because it closes the moment a combination is read and the
                // user's fingers are still down at that point. A release let through here for a
                // press that was swallowed is a key that sticks down in every other application.
                if !down && SWALLOWED.with(|l| l.borrow_mut().owes_release(kb.vkCode)) {
                    OWED_RELEASES_TAKEN.fetch_add(1, Ordering::Relaxed);
                    return LRESULT(1);
                }
                if !capturing && on_physical_key(kb.vkCode, down) {
                    // Taken: the combination is ours, so the key must not also reach whatever the
                    // user is typing into. Returning non-zero ends the chain here.
                    return LRESULT(1);
                }
            }
        }
    }
    // SAFETY: forwarding the exact arguments we received, as the hook contract requires.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Edge detection for one physical key transition (hook thread only).
///
/// Returns whether the event should be swallowed rather than passed to the rest of the system.
fn on_physical_key(vk_code: u32, down: bool) -> bool {
    TRACKER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let generation = COMBO_GEN.load(Ordering::Acquire);
        if slot.0 != generation {
            *slot = (generation, ComboTracker::new(load_combo()));
            // A new combination starts with nothing taken, or the old trigger's key-up would be
            // swallowed on behalf of a combination that no longer exists.
            SWALLOWING.with(|s| s.set(false));
        }
        let combo = slot.1.combo();
        if combo.is_empty() {
            return false;
        }
        let registered = USE_REGISTERED.load(Ordering::Acquire);
        if let Some(event) = slot.1.on_key(vk_code, down) {
            COMBO_ACTIVE.store(event == HotkeyEvent::Pressed, Ordering::Release);
            // When the window manager owns the combination it owns both of its edges -- see
            // `watch_for_release`. The hook must stay quiet or every press would be announced
            // twice, and dictation would start and immediately stop.
            if !registered {
                // Pre-allocated bounded channel: try_send never allocates; drop on overflow
                // rather than ever blocking the hook.
                let _ = events_channel().0.try_send(event);
            }
        }
        // Nothing to swallow when the window manager has the key: it never reached the hook
        // chain's consumers in the first place.
        if registered {
            return false;
        }
        let active = slot.1.is_active();
        SWALLOWING.with(|flag| {
            let mut swallowing = flag.get();
            let take = swallow_trigger(&combo, active, &mut swallowing, vk_code, down);
            flag.set(swallowing);
            take
        })
    })
}

// ---------------------------------------------------------------------------
// The shortcut editor's keyboard grab
// ---------------------------------------------------------------------------

/// The grab is off; the hotkey has the keyboard as usual.
const CAPTURE_OFF: u32 = 0;
/// The grab is on and waiting for a combination.
///
/// There used to be a third state, held after a combination was read so that the releases still
/// to come could be swallowed. [`SwallowLedger`] does that job exactly instead of approximately,
/// and it does it without keeping the whole keyboard for another two seconds -- during which any
/// *other* key the user pressed was eaten as well, which is what made keys feel stuck.
const CAPTURE_ARMED: u32 = 1;

/// Which of the three the grab is in. Read by the hook callback on every key.
static CAPTURE_STATE: AtomicU32 = AtomicU32::new(CAPTURE_OFF);

/// Bumped when a session starts, so the hook thread rebuilds its tracker with all keys up.
static CAPTURE_GEN: AtomicU32 = AtomicU32::new(0);

/// When the current session gives up, in [`now_ms`] milliseconds.
///
/// A capture takes *every* key in the system, so an abandoned one would look exactly like a
/// machine whose keyboard had died. The window is closed from the hook itself rather than from a
/// timer, because the hook is the only thing that is certainly still running.
static CAPTURE_DEADLINE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How long the editor may hold the keyboard before it lets go by itself.
///
/// Every key in the system is taken for this long, so the number is not "how long might someone
/// take to decide" but "how long may a keyboard that appears dead stay that way". Someone who has
/// clicked *press the combination you want* presses it within a second or two; someone who has
/// wandered off wants their keyboard back. Seven seconds serves the first and does not punish the
/// second.
const CAPTURE_WINDOW_MS: u64 = 7_000;

static CAPTURE_EVENTS: OnceLock<(Sender<Captured>, Receiver<Captured>)> = OnceLock::new();

fn capture_channel() -> &'static (Sender<Captured>, Receiver<Captured>) {
    CAPTURE_EVENTS.get_or_init(|| bounded(8))
}

thread_local! {
    /// (session generation, tracker) — lives on the hook thread only.
    static CAPTURE: RefCell<(u32, CaptureTracker)> =
        RefCell::new((u32::MAX, CaptureTracker::new()));
    /// Presses the grab has taken, so their releases are taken too.
    ///
    /// Deliberately outside the generation check above: a session ends when a combination is
    /// read, and the releases it owes arrive *after* that. Resetting this with the session is
    /// exactly how a modifier ends up stuck down in every other application.
    static SWALLOWED: RefCell<SwallowLedger> = RefCell::new(SwallowLedger::new());
}

/// How many owed releases have been taken. Read by the live tests, which cannot see a
/// `thread_local` on the hook thread.
static OWED_RELEASES_TAKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One key transition while the editor holds the keyboard (hook thread only).
///
/// Returns whether to swallow it, which is "yes" for as long as the grab lasts: a capture that let
/// the keys through would open the Start menu the moment someone pressed the Windows key, and
/// leave the character in whatever was behind the settings window.
fn on_capture_key(vk_code: u32, down: bool) -> bool {
    GRAB_KEYS_SEEN.fetch_add(1, Ordering::Relaxed);
    if now_ms() > CAPTURE_DEADLINE_MS.load(Ordering::Acquire) {
        finish_capture();
        return false;
    }
    CAPTURE.with(|cell| {
        let mut slot = cell.borrow_mut();
        let generation = CAPTURE_GEN.load(Ordering::Acquire);
        if slot.0 != generation {
            *slot = (generation, CaptureTracker::new());
        }
        if let Some(captured) = slot.1.on_key(vk_code, down) {
            // Pre-allocated bounded channel, as everywhere else on this thread.
            let _ = capture_channel().0.try_send(captured);
            // The keyboard goes back at once. What is still owed is the releases of the keys
            // whose presses were taken, and the ledger settles those on its own.
            finish_capture();
        }
        true
    })
}

/// Arm the grab. Any result from an abandoned session is dropped first.
fn start_capture() {
    while capture_channel().1.try_recv().is_ok() {}
    CAPTURE_GEN.fetch_add(1, Ordering::Release);
    CAPTURE_DEADLINE_MS.store(now_ms() + CAPTURE_WINDOW_MS, Ordering::Release);
    CAPTURE_STATE.store(CAPTURE_ARMED, Ordering::Release);
}

/// End the grab and hand the keyboard back to the hotkey with nothing held.
///
/// The combo tracker sat out the whole session, so its idea of what is down is whatever was true
/// when the session began. Bumping the generation makes the hook rebuild it all-keys-up, which is
/// the only state that is certainly true: a modifier held while the grab started and released
/// during it would otherwise read as still down for ever.
fn finish_capture() {
    if CAPTURE_STATE.swap(CAPTURE_OFF, Ordering::AcqRel) == CAPTURE_OFF {
        return;
    }
    COMBO_GEN.fetch_add(1, Ordering::Release);
    if COMBO_ACTIVE.swap(false, Ordering::AcqRel) {
        let _ = events_channel().0.try_send(HotkeyEvent::Released);
    }
}

/// Put the hook back and wait for the hook thread to say it has, within reason.
///
/// Bounded, and a timeout is not fatal: the worst case is the hook the process already had, which
/// is the one every other part of the app is using anyway. Blocking for ever on a thread that has
/// stopped answering would turn a stale hook into a frozen window.
fn rehook_and_wait() {
    let before = REHOOK_GEN.load(Ordering::Acquire);
    post_to_hook_thread(WM_REHOOK);
    let deadline = std::time::Instant::now() + Duration::from_millis(250);
    while REHOOK_GEN.load(Ordering::Acquire) == before {
        if std::time::Instant::now() >= deadline {
            tracing::warn!("the hook thread did not confirm the re-install; arming anyway");
            return;
        }
        std::thread::yield_now();
    }
}

/// The shortcut editor's keyboard grab, backed by the process-wide hook.
///
/// One at a time: a second session re-arms the same grab, and whichever handle is dropped first
/// ends it. The editor only ever opens one.
pub struct WindowsCapture {
    _private: (),
}

impl WindowsCapture {
    /// Arm the grab, installing (or attaching to) the keyboard hook first.
    pub fn new() -> Result<Self> {
        ensure_hook()?;
        rehook_and_wait();
        start_capture();
        tracing::debug!("shortcut capture armed");
        Ok(Self { _private: () })
    }
}

impl HotkeyCapture for WindowsCapture {
    fn events(&self) -> Receiver<Captured> {
        capture_channel().1.clone()
    }

    fn key_counts(&self) -> (u64, u64) {
        key_counts()
    }
}

impl Drop for WindowsCapture {
    fn drop(&mut self) {
        // Safe at any moment, including mid-gesture: the releases still owed are settled by the
        // ledger, which does not belong to the session and does not end with it.
        finish_capture();
        tracing::debug!("shortcut capture released");
    }
}

impl std::fmt::Debug for WindowsCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsCapture")
            .field("state", &CAPTURE_STATE.load(Ordering::Relaxed))
            .finish()
    }
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
    LAST_EVENT_MS.store(now_ms(), Ordering::Relaxed);
    // PostThreadMessageW fails if the target has not created a message queue. The pump can
    // register immediately after the ready signal, so create the queue before sending it.
    let mut msg = MSG::default();
    // SAFETY: this non-removing peek initializes the current thread's message queue.
    let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
    // SAFETY: no preconditions; returns the calling thread's id.
    HOOK_THREAD.store(
        unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
        Ordering::Release,
    );
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
    // SAFETY: msg is a valid, owned MSG; standard blocking message pump. A return of -1 is an
    // error and 0 is WM_QUIT; both end the loop.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        if msg.message == WM_TIMER {
            hook = watchdog(hook);
            continue;
        }
        if msg.message == WM_REBIND {
            let _ = rebind_ack()
                .0
                .send((msg.wParam.0 as u32, rebind_with_window_manager()));
            continue;
        }
        if msg.message == WM_REHOOK {
            hook = reinstall(hook);
            REHOOK_GEN.fetch_add(1, Ordering::Release);
            continue;
        }
        if msg.message == WM_HOTKEY && msg.wParam.0 as i32 == HOTKEY_ID {
            // Windows says the combination was pressed. It has already taken the key from
            // whatever had focus, so there is nothing to swallow and nothing to check.
            COMBO_ACTIVE.store(true, Ordering::Release);
            let _ = events_channel().0.try_send(HotkeyEvent::Pressed);
            // The release will not arrive on its own: a registered hotkey reports key-down only.
            watch_for_release(load_combo());
            continue;
        }
        // SAFETY: msg came from GetMessageW and is valid for these calls.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// How often the watcher looks at the keyboard while the combination is held down.
///
/// Fifteen milliseconds: far below the shortest deliberate tap, and 66 wake-ups a second on one
/// thread that exists only while a key is actually held.
const RELEASE_POLL_MS: u64 = 15;

/// A hold this long is taken as a key that never reported itself up.
///
/// Push-to-talk is a thumb on a key; ten minutes of it is not a dictation, it is a watcher that
/// has been stranded, and leaving it spinning for the life of the process would be worse than
/// ending the utterance.
const LONGEST_HOLD: Duration = Duration::from_secs(600);

/// Whether a press is already being watched, so a repeat cannot start a second watcher.
static WATCHING_RELEASE: AtomicBool = AtomicBool::new(false);

/// Is every part of `combo` held down right now?
fn combo_is_held(combo: &ParsedCombo) -> bool {
    fn down(vk: i32) -> bool {
        // SAFETY: no preconditions; reads the asynchronous state of one virtual key.
        (unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk) } as u16
            & 0x8000)
            != 0
    }
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
    if combo.trigger != 0 && !down(i32::from(combo.trigger)) {
        return false;
    }
    if combo.mods & MOD_CTRL != 0 && !down(VK_CONTROL.0 as i32) {
        return false;
    }
    if combo.mods & MOD_SHIFT != 0 && !down(VK_SHIFT.0 as i32) {
        return false;
    }
    if combo.mods & MOD_ALT != 0 && !down(VK_MENU.0 as i32) {
        return false;
    }
    if combo.mods & MOD_WIN != 0 && !down(VK_LWIN.0 as i32) && !down(VK_RWIN.0 as i32) {
        return false;
    }
    true
}

/// Watch a registered combination until it is let go, and announce the release.
///
/// `RegisterHotKey` reports the press and nothing else; push-to-talk is defined by the release, so
/// the other half has to come from somewhere. It used to come from the keyboard hook, and that is
/// the wrong place to get it: a low-level hook is the one part of this that another process can
/// starve, that Windows removes silently when a callback runs long, and that -- measured on this
/// machine, with a twenty-line hook of its own in a separate process -- does not see synthesized
/// keys at all. Asking the keyboard directly asks nobody's permission and cannot be taken away.
///
/// The thread exists only while a key is down.
fn watch_for_release(combo: ParsedCombo) {
    if WATCHING_RELEASE.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("lw-hotkey-release".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            while combo_is_held(&combo) {
                if started.elapsed() > LONGEST_HOLD {
                    tracing::warn!("the hotkey has read as held for ten minutes; letting it go");
                    break;
                }
                std::thread::sleep(Duration::from_millis(RELEASE_POLL_MS));
            }
            COMBO_ACTIVE.store(false, Ordering::Release);
            let _ = events_channel().0.try_send(HotkeyEvent::Released);
            WATCHING_RELEASE.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        // Without a watcher there is no release edge, and push-to-talk would hold the microphone
        // open forever. Say so rather than record until the process ends.
        tracing::error!("could not watch the hotkey for its release; push-to-talk will not stop");
        WATCHING_RELEASE.store(false, Ordering::Release);
    }
}

/// Hand the current combination to the window manager, on the hook thread.
///
/// Called only from that thread: `RegisterHotKey` binds the registration to whoever calls it, and
/// `WM_HOTKEY` then arrives in that thread's queue.
fn rebind_with_window_manager() -> std::result::Result<(), String> {
    // SAFETY: unregistering our own id; harmless when nothing is registered.
    unsafe {
        let _ = UnregisterHotKey(None, HOTKEY_ID);
    }
    USE_REGISTERED.store(false, Ordering::Release);

    let combo = load_combo();
    if combo.is_empty() {
        return Ok(());
    }
    if combo.trigger == 0 {
        // Modifiers on their own: `RegisterHotKey` will not take them, and the hook is the only
        // way. Not a failure -- the binding still works, by the other path.
        tracing::info!("modifier-only binding: the keyboard hook will detect it");
        return Ok(());
    }

    // `MOD_NOREPEAT`: holding the keys down must not fire over and over. The hook's own tracker
    // already ignores auto-repeat, and the two must agree.
    let mut mods = WIN_MOD_NOREPEAT;
    if combo.mods & crate::hotkey::MOD_CTRL != 0 {
        mods |= WIN_MOD_CONTROL;
    }
    if combo.mods & crate::hotkey::MOD_SHIFT != 0 {
        mods |= WIN_MOD_SHIFT;
    }
    if combo.mods & crate::hotkey::MOD_ALT != 0 {
        mods |= WIN_MOD_ALT;
    }
    if combo.mods & crate::hotkey::MOD_WIN != 0 {
        mods |= WIN_MOD_WIN;
    }

    // SAFETY: a thread-scoped registration with our own id; the result is checked.
    match unsafe { RegisterHotKey(None, HOTKEY_ID, mods, u32::from(combo.trigger)) } {
        Ok(()) => {
            USE_REGISTERED.store(true, Ordering::Release);
            tracing::info!("hotkey registered with the window manager");
            Ok(())
        }
        Err(e) => {
            tracing::warn!("the window manager refused this combination: {e}");
            Err(format!("Windows could not register this shortcut: {e}"))
        }
    }
}

/// Install the hook, returning its handle.
fn install_hook() -> std::result::Result<HHOOK, String> {
    // SAFETY: no preconditions; the id of the thread making the call.
    let thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    // SAFETY: installing a global WH_KEYBOARD_LL hook with a valid callback; hmod may be
    // null for LL hooks (the callback lives in this module, not a DLL).
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_kbd_proc), None, 0) }
        .map_err(|e| format!("SetWindowsHookExW(WH_KEYBOARD_LL) failed: {e}"))?;
    // The installing thread is the one the callbacks are delivered to, and it must be the one
    // parked in `GetMessageW`. Recorded because a hook installed from anywhere else is a hook
    // that never fires, and nothing else about it looks wrong from the outside.
    tracing::info!(?hook, installed_by_thread = thread, "keyboard hook installed");
    Ok(hook)
}

/// Re-install the hook if nothing has been seen through it for a while.
///
/// There is no API that answers "is my hook still installed", so silence is the only signal
/// available -- and silence is ambiguous: it also means nobody is typing. Re-installing in that
/// case costs two calls and nothing else, which is a good trade against a hotkey that is dead for
/// the rest of the session.
fn watchdog(current: HHOOK) -> HHOOK {
    // Silence is ambiguous -- a hook that has been removed and a keyboard nobody is touching look
    // exactly alike -- so this only acts while the hook is the thing that has to deliver. When the
    // window manager holds the combination the press arrives as `WM_HOTKEY` and the release from
    // `watch_for_release`, and re-installing on every quiet stretch would cost keystrokes for
    // nothing: there is a window between unhook and hook in which events reach no one, and a
    // measured 20-second run of the live tests loses a press edge to it.
    //
    // That leaves the shortcut editor, which needs the hook whatever the binding is. It does not
    // wait for this: arming the grab re-installs the hook itself, at the one moment when the hook
    // is known to be needed. See `rehook_and_wait`.
    if USE_REGISTERED.load(Ordering::Acquire) {
        return current;
    }
    let quiet = now_ms().saturating_sub(LAST_EVENT_MS.load(Ordering::Relaxed));
    if quiet < u64::from(WATCHDOG_MS) {
        return current;
    }
    let (seen, _) = key_counts();
    tracing::info!(keys_seen_so_far = seen, "no key through the hook for {quiet} ms; re-installing");
    reinstall(current)
}

/// Take the hook down and put it straight back, on the hook thread.
///
/// There is no asking Windows whether a low-level hook is still installed, so the only way to be
/// sure of one is to install it again. Two syscalls, and the handle the caller must keep.
fn reinstall(current: HHOOK) -> HHOOK {
    // SAFETY: `current` came from SetWindowsHookExW on this thread and is unhooked once.
    unsafe {
        let _ = UnhookWindowsHookEx(current);
    }
    match install_hook() {
        Ok(h) => {
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
/// Only one combo is active at a time process-wide (OwlWhisp needs exactly one
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
        if let Err(e) = rebind_and_wait() {
            // The caller will treat this binding as absent. Clear the hook's copy too, or a
            // failed registration could still fire the old listener behind that status.
            Self::publish_combo(ParsedCombo::default());
            let _ = rebind_and_wait();
            return Err(e);
        }
        self.registered = true;
        tracing::info!(?spec, "hotkey registered");
        Ok(())
    }

    fn unregister(&mut self) -> Result<()> {
        if self.registered {
            Self::publish_combo(ParsedCombo::default());
            rebind_and_wait()?;
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

    /// The shortcut editor's grab, for real: arm it, press Win+F13, and see it come back.
    ///
    /// This is the case the old, window-level capture could not do at all, and the reason the grab
    /// exists. It cannot be tested without a hook, because the whole claim is about what the hook
    /// sees that a window does not.
    ///
    /// Run with
    /// `cargo test -p lw-platform --lib live_tests::the_grab_reads_the_windows_key -- --ignored`.
    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    fn the_grab_reads_the_windows_key() {
        const VK_LWIN: u16 = 0x5B;
        const VK_F13: u16 = 0x7C;

        let grab = WindowsCapture::new().expect("the hook installs");
        let events = grab.events();

        key(VK_LWIN, false);
        key(VK_F13, false);
        let got = events.recv_timeout(Duration::from_millis(1500));
        key(VK_F13, true);
        key(VK_LWIN, true);
        drop(grab);

        assert_eq!(
            got,
            Ok(Captured::Combo(crate::hotkey::RawCombo {
                mods: MOD_WIN,
                vk: VK_F13,
            })),
        );
        assert_eq!(crate::hotkey::vk_name(VK_F13).as_deref(), Some("f13"));
    }

    /// Every modifier held is part of what the grab reads, including the first one pressed.
    ///
    /// This is a regression test with a date on it. Arming the grab puts the keyboard hook back
    /// first, and that used to be a message posted to the hook thread and not waited for -- so a
    /// key pressed in the microseconds while the hook was down was seen by nobody, and Ctrl+Meta+D
    /// came back as Meta+D, or as nothing at all. Pressing immediately after arming, which is what
    /// this does, is exactly the case that broke.
    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    fn no_modifier_is_lost_between_arming_the_grab_and_the_first_key() {
        const VK_LCONTROL: u16 = 0xA2;
        const VK_LWIN: u16 = 0x5B;
        const VK_F13: u16 = 0x7C;

        for _ in 0..5 {
            let grab = WindowsCapture::new().expect("the hook installs");
            let events = grab.events();
            for k in [VK_LCONTROL, VK_LWIN, VK_F13] {
                key(k, false);
            }
            let got = events.recv_timeout(Duration::from_millis(1500));
            for k in [VK_F13, VK_LWIN, VK_LCONTROL] {
                key(k, true);
            }
            drop(grab);
            assert_eq!(
                got,
                Ok(Captured::Combo(crate::hotkey::RawCombo {
                    mods: MOD_CTRL | MOD_WIN,
                    vk: VK_F13,
                })),
            );
        }
    }

    /// The grab, in a process that already has a hotkey registered with the window manager.
    ///
    /// That is the running application, and it is not the same process the other grab tests
    /// describe: `RegisterHotKey` has succeeded, `USE_REGISTERED` is set, and the hook is on
    /// nobody's path until the editor wants it. If the grab only works in a process that has
    /// never registered anything, it does not work.
    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    fn the_grab_works_beside_a_hotkey_the_window_manager_holds() {
        const VK_LWIN: u16 = 0x5B;
        const VK_F13: u16 = 0x7C;

        // Not the application's own binding: a combination another process already registered
        // cannot be registered here, and the test would then prove nothing.
        let mut hotkey = WindowsHotkey::new().expect("the hook installs");
        hotkey
            .register(&HotkeySpec {
                modifiers: vec!["ctrl".into()],
                trigger: "f12".into(),
            })
            .expect("ctrl+f12 registers");
        // `register` must wait for the hook thread's actual Windows registration.
        assert!(
            USE_REGISTERED.load(Ordering::Acquire),
            "this test is meaningless unless the window manager took the hotkey",
        );

        let grab = WindowsCapture::new().expect("the hook installs");
        let events = grab.events();
        key(VK_LWIN, false);
        key(VK_F13, false);
        let got = events.recv_timeout(Duration::from_millis(1500));
        key(VK_F13, true);
        key(VK_LWIN, true);
        drop(grab);
        let _ = hotkey.unregister();

        assert_eq!(
            got,
            Ok(Captured::Combo(crate::hotkey::RawCombo {
                mods: MOD_WIN,
                vk: VK_F13,
            })),
        );
    }

    /// Two modifiers and no key at all -- the Ctrl+Meta push-to-talk, read off the keyboard.
    ///
    /// Settled on the way *up*, which is what tells it apart from a user still on their way to
    /// Ctrl+Meta+D, and why the grab has to stay on the keyboard after the last key-down.
    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    fn the_grab_reads_two_modifiers_with_no_key() {
        const VK_LCONTROL: u16 = 0xA2;
        const VK_LWIN: u16 = 0x5B;

        let grab = WindowsCapture::new().expect("the hook installs");
        let events = grab.events();

        key(VK_LCONTROL, false);
        key(VK_LWIN, false);
        assert!(
            events.try_recv().is_err(),
            "nothing is decided while the keys are still down",
        );
        key(VK_LWIN, true);
        key(VK_LCONTROL, true);
        let got = events.recv_timeout(Duration::from_millis(1500));
        drop(grab);

        assert_eq!(
            got,
            Ok(Captured::Combo(crate::hotkey::RawCombo {
                mods: MOD_CTRL | MOD_WIN,
                vk: 0,
            })),
        );
        assert_eq!(crate::hotkey::vk_name(0).as_deref(), Some("none"));
    }

    /// Whatever the grab took on the way down, it takes on the way up -- after it has let go.
    ///
    /// This is the stuck-key test. The grab releases the keyboard the instant it has read a
    /// combination, which is while the user's fingers are still down; the releases that follow
    /// belong to presses no other application ever saw, and letting them through leaves Ctrl held
    /// down everywhere. The handle is dropped here too, exactly as the editor drops it, to show
    /// that the obligation does not die with the session.
    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    fn the_releases_of_swallowed_presses_are_swallowed_after_the_grab_has_let_go() {
        const VK_LCONTROL: u16 = 0xA2;
        const VK_F13: u16 = 0x7C;

        let before = OWED_RELEASES_TAKEN.load(Ordering::Relaxed);
        let grab = WindowsCapture::new().expect("the hook installs");
        let events = grab.events();

        key(VK_LCONTROL, false);
        key(VK_F13, false);
        assert!(events.recv_timeout(Duration::from_millis(1500)).is_ok());
        assert_eq!(
            CAPTURE_STATE.load(Ordering::Acquire),
            CAPTURE_OFF,
            "the keyboard goes back as soon as the combination is read, not seconds later",
        );
        drop(grab);

        key(VK_F13, true);
        key(VK_LCONTROL, true);
        for _ in 0..100 {
            if OWED_RELEASES_TAKEN.load(Ordering::Relaxed) >= before + 2 {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the releases of the swallowed presses reached the rest of the system");
    }

    /// A key already held when the grab opens keeps its own release.
    ///
    /// The other half of the same rule, and the one that actually sticks keys: the application in
    /// front saw that press, so taking the release would leave it holding a key for ever. Nothing
    /// is owed for a press the grab never took.
    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    fn a_key_already_down_when_the_grab_opens_keeps_its_release() {
        const VK_LSHIFT: u16 = 0xA0;
        const VK_F13: u16 = 0x7C;

        key(VK_LSHIFT, false);
        let before = OWED_RELEASES_TAKEN.load(Ordering::Relaxed);
        let grab = WindowsCapture::new().expect("the hook installs");
        let events = grab.events();
        key(VK_F13, false);
        let _ = events.recv_timeout(Duration::from_millis(1500));
        key(VK_F13, true);
        key(VK_LSHIFT, true);
        drop(grab);
        std::thread::sleep(Duration::from_millis(150));

        // One release owed (F13's, whose press the grab took) and not two: Shift went down before
        // the grab existed, so its release was never the grab's to take.
        assert_eq!(
            OWED_RELEASES_TAKEN.load(Ordering::Relaxed),
            before + 1,
            "the grab took the release of a key it never took the press of",
        );
    }

    /// Press `keys` in order, let go in reverse, and report what the shortcut editor's grab read.
    fn capture_probe(keys: &[u16]) -> Option<Captured> {
        let grab = WindowsCapture::new().ok()?;
        let events = grab.events();
        for k in keys {
            key(*k, false);
        }
        let first = events.recv_timeout(Duration::from_millis(800)).ok();
        for k in keys.iter().rev() {
            key(*k, true);
        }
        // A modifiers-only gesture is only settled on the way up.
        let result = first.or_else(|| events.recv_timeout(Duration::from_millis(800)).ok());
        drop(grab);
        result
    }

    /// What the grab reads for each shape of combination, printed rather than asserted.
    ///
    /// Every combination here is built from `f13`/`f14`, which no keyboard has and nothing in
    /// Windows listens for. That restriction is the whole design of this test and not an
    /// incidental choice: it injects into the *live* session, so a combination the shell
    /// understands is one the shell carries out. An earlier version of this asked whether the
    /// hook is offered `win+d` and `win+space` before the shell is, and the answer cost a real
    /// user a new virtual desktop, every window minimised and a switched keyboard layout. What
    /// the hook sees does not depend on which key completes the combination, so the question is
    /// answerable without touching a single key the shell knows.
    ///
    /// `cargo test -p lw-platform --lib live_tests::report_what_the_grab_reads -- --ignored --nocapture`
    #[test]
    #[ignore = "installs a real keyboard hook and injects keystrokes"]
    fn report_what_the_grab_reads() {
        const LWIN: u16 = 0x5B;
        const LCTRL: u16 = 0xA2;
        const LSHIFT: u16 = 0xA0;
        const F13: u16 = 0x7C;
        const F14: u16 = 0x7D;

        for (name, keys) in [
            ("Ctrl+F13", &[LCTRL, F13][..]),
            ("Meta+F13", &[LWIN, F13][..]),
            ("Meta+F14", &[LWIN, F14][..]),
            ("Ctrl+Meta+F13", &[LCTRL, LWIN, F13][..]),
            ("Ctrl+Shift+F14", &[LCTRL, LSHIFT, F14][..]),
            ("Ctrl+Meta (no key)", &[LCTRL, LWIN][..]),
        ] {
            let read = capture_probe(keys);
            let shown = match read {
                Some(Captured::Combo(c)) => format!(
                    "mods={:#06b} key={}",
                    c.mods,
                    crate::hotkey::vk_name(c.vk).unwrap_or_else(|| format!("{:#x}", c.vk)),
                ),
                Some(Captured::Cancelled) => "cancelled".into(),
                None => "NOTHING READ".into(),
            };
            println!("{name:<22} {shown}");
        }
    }

    /// Inject a combination this process is **not** bound to, so it passes through the chain.
    ///
    /// The point is to drive another running application's hotkey from here. A hook that matches a
    /// combination now swallows it, which is right for a global hotkey and makes this process
    /// useless as a driver unless it deliberately registers something else.
    ///
    /// Run it with `--nocapture` and watch the application's log:
    /// `cargo test -p lw-platform --lib live_tests::press_ctrl_space_for_another_app -- --ignored`
    #[test]
    #[ignore = "injects keystrokes for whatever else is running"]
    fn press_ctrl_space_for_another_app() {
        const VK_CONTROL: u16 = 0x11;
        const VK_SPACE: u16 = 0x20;

        let mut hotkey = WindowsHotkey::new().expect("install the keyboard hook");
        let events = hotkey.events();
        // Something nothing else uses, so this hook watches without taking anything.
        hotkey
            .register(&HotkeySpec {
                modifiers: vec!["ctrl".into(), "alt".into(), "shift".into()],
                trigger: "f20".into(),
            })
            .expect("register a combination of our own");
        while events.try_recv().is_ok() {}

        key(VK_CONTROL, false);
        key(VK_SPACE, false);
        std::thread::sleep(Duration::from_millis(120));
        key(VK_SPACE, true);
        key(VK_CONTROL, true);
        std::thread::sleep(Duration::from_millis(200));

        assert!(
            events.try_recv().is_err(),
            "our own hook matched, so the keys never travelled on"
        );
        println!("injected ctrl+space; check the other application");
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
            // Meta combinations, on keys nothing owns. Never `win+d`, `win+space` or anything
            // else the shell acts on: this test injects into the live session, and a combination
            // the shell understands is a combination the shell *does*. Asking whether the hook
            // sees `win+d` once cost a real user a new virtual desktop, every window minimised
            // and a switched keyboard layout. `f13`/`f14` answer the same question about the hook
            // and mean nothing to anybody else.
            (&["win"][..], "f13"),
            (&["ctrl", "win"][..], "f14"),
            (&["ctrl", "win"][..], "none"),
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
