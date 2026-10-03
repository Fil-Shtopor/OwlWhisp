//! Global hotkeys: the platform-neutral trait, the spec parser, and the **pure**
//! combo edge-detection state machine.
//!
//! The Windows implementation (`crate::windows::WindowsHotkey`) feeds physical key
//! transitions from a `WH_KEYBOARD_LL` hook into [`ComboTracker`]; the tracker itself is
//! OS-free and unit-tested here with synthetic events.
//!
//! Key codes are Windows virtual-key codes (`vk`), used as the crate-wide neutral key id —
//! non-Windows backends translate their native codes into these.

use crossbeam_channel::Receiver;

use crate::{Error, Result};

/// A press/release edge of the configured combo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// The full combo just became held (0 → 1 edge).
    Pressed,
    /// The combo just stopped being held (1 → 0 edge).
    Released,
}

/// User-facing hotkey description: modifier names plus an optional trigger key name.
///
/// Modifiers: `"ctrl"`, `"shift"`, `"alt"`, `"win"` (aka `"super"`/`"cmd"`/`"meta"`).
/// Trigger: a key name (see [`trigger_vk`]) or `""`/`"none"` for a modifiers-only combo
/// (e.g. Ctrl+Win push-to-talk).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HotkeySpec {
    /// Modifier names, e.g. `["ctrl", "win"]`.
    pub modifiers: Vec<String>,
    /// Trigger key name, or `""`/`"none"` for modifiers-only.
    pub trigger: String,
}

/// Platform-neutral global hotkey listener.
pub trait GlobalHotkey: Send {
    /// Start (or replace) detection of `spec`. Emits [`HotkeyEvent`]s on the channel
    /// returned by [`events`](GlobalHotkey::events).
    fn register(&mut self, spec: &HotkeySpec) -> Result<()>;
    /// Stop detection. If the combo is currently held a final `Released` is emitted.
    fn unregister(&mut self) -> Result<()>;
    /// The event channel. Cloning the receiver is allowed; events are delivered to one
    /// receiver each (crossbeam channels are multi-consumer, not broadcast).
    fn events(&self) -> Receiver<HotkeyEvent>;
}

// ---------------------------------------------------------------------------
// Modifier families and VK codes (pure data, cross-platform)
// ---------------------------------------------------------------------------

/// Bit for the Ctrl modifier family in [`ParsedCombo::mods`].
pub const MOD_CTRL: u8 = 1 << 0;
/// Bit for the Shift modifier family.
pub const MOD_SHIFT: u8 = 1 << 1;
/// Bit for the Alt modifier family.
pub const MOD_ALT: u8 = 1 << 2;
/// Bit for the Win (super) modifier family.
pub const MOD_WIN: u8 = 1 << 3;

/// Virtual-key codes used by the combo engine (Windows values, crate-neutral ids).
pub mod vk {
    /// Generic Shift.
    pub const SHIFT: u16 = 0x10;
    /// Generic Ctrl.
    pub const CONTROL: u16 = 0x11;
    /// Generic Alt (VK_MENU).
    pub const MENU: u16 = 0x12;
    /// Left Shift.
    pub const LSHIFT: u16 = 0xA0;
    /// Right Shift.
    pub const RSHIFT: u16 = 0xA1;
    /// Left Ctrl.
    pub const LCONTROL: u16 = 0xA2;
    /// Right Ctrl.
    pub const RCONTROL: u16 = 0xA3;
    /// Left Alt.
    pub const LMENU: u16 = 0xA4;
    /// Right Alt.
    pub const RMENU: u16 = 0xA5;
    /// Left Windows key.
    pub const LWIN: u16 = 0x5B;
    /// Right Windows key.
    pub const RWIN: u16 = 0x5C;
    /// Return / Enter.
    pub const RETURN: u16 = 0x0D;
    /// Escape.
    pub const ESCAPE: u16 = 0x1B;
}

/// The modifier family a virtual-key code belongs to, if any.
pub fn vk_family(code: u16) -> Option<u8> {
    match code {
        vk::CONTROL | vk::LCONTROL | vk::RCONTROL => Some(MOD_CTRL),
        vk::SHIFT | vk::LSHIFT | vk::RSHIFT => Some(MOD_SHIFT),
        vk::MENU | vk::LMENU | vk::RMENU => Some(MOD_ALT),
        vk::LWIN | vk::RWIN => Some(MOD_WIN),
        _ => None,
    }
}

/// Map a modifier name to its family bit.
pub fn modifier_bit(name: &str) -> Option<u8> {
    match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(MOD_CTRL),
        "shift" => Some(MOD_SHIFT),
        "alt" | "option" => Some(MOD_ALT),
        "win" | "super" | "cmd" | "meta" => Some(MOD_WIN),
        _ => None,
    }
}

/// Map a trigger key name to its virtual-key code. `""` and `"none"` mean
/// "no trigger" (modifiers-only combo) and map to `Some(0)`.
pub fn trigger_vk(name: &str) -> Option<u16> {
    let n = name.to_ascii_lowercase();
    let code = match n.as_str() {
        "" | "none" => 0,
        "space" => 0x20,
        "tab" => 0x09,
        "enter" | "return" => 0x0D,
        "escape" | "esc" => 0x1B,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "page_up" | "pageup" => 0x21,
        "page_down" | "pagedown" => 0x22,
        "caps_lock" | "capslock" => 0x14,
        "scroll_lock" | "scrolllock" => 0x91,
        "pause" => 0x13,
        "print_screen" | "printscreen" => 0x2C,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "right_ctrl" => vk::RCONTROL,
        "right_shift" => vk::RSHIFT,
        "right_alt" => vk::RMENU,
        "grave" | "backquote" | "`" => 0xC0,
        _ => {
            // f1..f24
            if let Some(num) = n.strip_prefix('f').and_then(|s| s.parse::<u16>().ok())
                && (1..=24).contains(&num)
            {
                return Some(0x70 + num - 1);
            }
            // single letter or digit
            let mut chars = n.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if c.is_ascii_lowercase() {
                    return Some(c.to_ascii_uppercase() as u16);
                }
                if c.is_ascii_digit() {
                    return Some(c as u16);
                }
            }
            return None;
        }
    };
    Some(code)
}

/// The name a settings file uses for a virtual-key code, if this build has one for it.
///
/// The partial inverse of [`trigger_vk`], and deliberately *narrower* than it: only the keys a
/// saved binding is known to survive are named here. The shortcut editor captures whatever the
/// keyboard sends, and a name the rest of the app would later refuse is worse than no name --
/// it would be shown as bound and then fail to save, with nothing on screen saying why.
///
/// `None` therefore means "this key cannot be part of a shortcut", which is a sentence the editor
/// can show. It is what a laptop's media keys give, and what the Fn key gives on the rare keyboard
/// that reports it at all.
pub fn vk_name(code: u16) -> Option<String> {
    let name = match code {
        // Not a key at all: the name a modifiers-only binding writes into the settings file.
        0 => "none",
        0x20 => "space",
        0x09 => "tab",
        vk::RETURN => "enter",
        vk::ESCAPE => "esc",
        0x14 => "capslock",
        0x2D => "insert",
        0xC0 => "backquote",
        0x41..=0x5A => return Some(((code as u8) as char).to_ascii_lowercase().to_string()),
        0x30..=0x39 => return Some(((code as u8) as char).to_string()),
        // f1..f20: past f20 `lw-core` has no accelerator name, so neither has this.
        0x70..=0x83 => return Some(format!("f{}", code - 0x70 + 1)),
        _ => return None,
    };
    Some(name.to_string())
}

/// A parsed, validated combo: a modifier-family bitmask plus an optional trigger key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParsedCombo {
    /// Bitwise OR of [`MOD_CTRL`], [`MOD_SHIFT`], [`MOD_ALT`], [`MOD_WIN`].
    pub mods: u8,
    /// Trigger virtual-key code, or 0 for a modifiers-only combo.
    pub trigger: u16,
}

impl ParsedCombo {
    /// True if neither modifiers nor a trigger are configured (never fires).
    pub fn is_empty(&self) -> bool {
        self.mods == 0 && self.trigger == 0
    }
}

/// Parse and validate a [`HotkeySpec`].
///
/// Errors on unknown modifier/trigger names and on the empty combo (no modifiers, no
/// trigger), which could never fire.
pub fn parse_spec(spec: &HotkeySpec) -> Result<ParsedCombo> {
    let mut mods = 0u8;
    for m in &spec.modifiers {
        mods |= modifier_bit(m).ok_or_else(|| Error::Hotkey(format!("unknown modifier {m:?}")))?;
    }
    let trigger = trigger_vk(&spec.trigger)
        .ok_or_else(|| Error::Hotkey(format!("unknown trigger key {:?}", spec.trigger)))?;
    let combo = ParsedCombo { mods, trigger };
    if combo.is_empty() {
        return Err(Error::Hotkey(
            "empty hotkey: no modifiers and no trigger key".into(),
        ));
    }
    Ok(combo)
}

// ---------------------------------------------------------------------------
// Pure edge-detection state machine
// ---------------------------------------------------------------------------

/// 256-bit pressed-key bitmap keyed by virtual-key code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyBitmap(pub [u64; 4]);

impl KeyBitmap {
    /// Record `code` as down/up. Codes above 255 are ignored.
    pub fn set(&mut self, code: u32, down: bool) {
        if code > 255 {
            return;
        }
        let word = (code / 64) as usize;
        let bit = 1u64 << (code % 64);
        if down {
            self.0[word] |= bit;
        } else {
            self.0[word] &= !bit;
        }
    }

    /// Whether `code` is currently recorded as down.
    pub fn is_down(&self, code: u32) -> bool {
        if code > 255 {
            return false;
        }
        self.0[(code / 64) as usize] & (1u64 << (code % 64)) != 0
    }

    /// Clear all keys.
    pub fn clear(&mut self) {
        self.0 = [0; 4];
    }
}

/// Whether the given modifier family has any of its keys (generic/left/right) down.
pub fn family_down(bits: &KeyBitmap, family: u8) -> bool {
    let codes: &[u16] = match family {
        MOD_CTRL => &[vk::CONTROL, vk::LCONTROL, vk::RCONTROL],
        MOD_SHIFT => &[vk::SHIFT, vk::LSHIFT, vk::RSHIFT],
        MOD_ALT => &[vk::MENU, vk::LMENU, vk::RMENU],
        MOD_WIN => &[vk::LWIN, vk::RWIN],
        _ => return false,
    };
    codes.iter().any(|&c| bits.is_down(c as u32))
}

/// Whether the combo is currently satisfied by the pressed-key bitmap.
///
/// Rules (strict matching):
/// - every configured modifier family must be down (any of generic/left/right);
/// - every **unconfigured** modifier family must be up — unless the trigger key itself
///   belongs to that family (e.g. a bare `right_ctrl` trigger);
/// - the trigger key, if configured, must be down;
/// - the empty combo is never active.
pub fn combo_active(bits: &KeyBitmap, combo: &ParsedCombo) -> bool {
    if combo.is_empty() {
        return false;
    }
    let trigger_family = vk_family(combo.trigger);
    for family in [MOD_CTRL, MOD_SHIFT, MOD_ALT, MOD_WIN] {
        let required = combo.mods & family != 0;
        let down = family_down(bits, family);
        if required && !down {
            return false;
        }
        if !required && down && trigger_family != Some(family) {
            return false;
        }
    }
    combo.trigger == 0 || bits.is_down(combo.trigger as u32)
}

/// Whether a key event belongs to the combo and must therefore not reach the focused application.
///
/// A global hotkey that also types into whatever is in front is not a global hotkey. Binding
/// Ctrl+Space and watching a space appear in the text box you were about to dictate into is the
/// clearest possible demonstration of that, and it is what the OS-level shortcut in the previous
/// build did for us: it consumed the key.
///
/// Only the **trigger** is swallowed, never a modifier. Swallowing Ctrl would break Ctrl+C in
/// every application on the machine for as long as this one runs -- an unacceptable price for a
/// dictation hotkey, and unnecessary, because the trigger alone is what produces the character.
///
/// `active_now` is whether the combo is satisfied *after* this event has been applied to the
/// tracker; `swallowing` remembers that the key-down was taken, so the matching key-up is taken
/// too. Without that, applications see a key-up with no key-down, and some treat it as a tap.
pub fn swallow_trigger(
    combo: &ParsedCombo,
    active_now: bool,
    swallowing: &mut bool,
    vk: u32,
    down: bool,
) -> bool {
    // A modifiers-only combo has no trigger to take, and taking a modifier is out of the question.
    if combo.trigger == 0 || vk != u32::from(combo.trigger) {
        return false;
    }
    if down {
        // `*swallowing` covers auto-repeat: the OS sends key-down over and over while the keys are
        // held, and every one of those would otherwise reach the application.
        if active_now || *swallowing {
            *swallowing = true;
            return true;
        }
        return false;
    }
    std::mem::take(swallowing)
}

/// Edge detector: feed physical key transitions, get [`HotkeyEvent`]s out.
///
/// Pure and allocation-free after construction — the Windows LL-hook callback drives one of
/// these on the hook thread; tests drive it with synthetic events.
#[derive(Clone, Copy, Debug)]
pub struct ComboTracker {
    combo: ParsedCombo,
    keys: KeyBitmap,
    active: bool,
}

impl ComboTracker {
    /// A tracker for `combo`, with all keys initially up.
    pub fn new(combo: ParsedCombo) -> Self {
        Self {
            combo,
            keys: KeyBitmap::default(),
            active: false,
        }
    }

    /// Whether the combo is currently held.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The combo this tracker detects.
    pub fn combo(&self) -> ParsedCombo {
        self.combo
    }

    /// Process one key transition; returns an event on a combo edge.
    pub fn on_key(&mut self, code: u32, down: bool) -> Option<HotkeyEvent> {
        self.keys.set(code, down);
        let want = combo_active(&self.keys, &self.combo);
        if want == self.active {
            return None;
        }
        self.active = want;
        Some(if want {
            HotkeyEvent::Pressed
        } else {
            HotkeyEvent::Released
        })
    }

    /// Forget all pressed keys (e.g. after a hook reinstall); returns `Released` if the
    /// combo was held.
    pub fn reset(&mut self) -> Option<HotkeyEvent> {
        self.keys.clear();
        if std::mem::take(&mut self.active) {
            Some(HotkeyEvent::Released)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Reading a combination off the keyboard, for the shortcut editor
// ---------------------------------------------------------------------------

/// A combination exactly as the keyboard delivered it: which modifier families were held, and
/// the key that completed it.
///
/// Raw codes rather than names because this is built inside the low-level hook callback, which
/// must not allocate. Naming happens on the other side of the channel, with [`vk_name`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawCombo {
    /// Bitwise OR of [`MOD_CTRL`], [`MOD_SHIFT`], [`MOD_ALT`], [`MOD_WIN`], as held at the moment
    /// the trigger went down.
    pub mods: u8,
    /// The non-modifier key that completed the combination.
    pub vk: u16,
}

/// What a capture session produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Captured {
    /// A combination. Whether it is *bindable* is [`vk_name`]'s answer, not this one's: the
    /// keyboard is entitled to send keys no shortcut can hold, and the editor should say so.
    Combo(RawCombo),
    /// Esc on its own — the user changed their mind.
    Cancelled,
}

/// Listening to the physical keyboard for the shortcut editor.
///
/// Separate from [`GlobalHotkey`] because it answers a different question. `GlobalHotkey` watches
/// for one known combination and stays out of everything else's way; this watches for *any* one
/// combination and takes every key while it does, so that arming it cannot open the Start menu or
/// type into whatever is behind the window.
///
/// Dropping the session stops the capture. Implementations also stop themselves after a short
/// while: a capture is a modal grab of the whole keyboard, and one that outlived the window that
/// started it would be indistinguishable from a machine that had stopped responding.
pub trait HotkeyCapture: Send {
    /// The channel the result arrives on. One event ends the session.
    fn events(&self) -> Receiver<Captured>;

    /// `(key transitions the backend has seen at all, transitions handed to the grab)`.
    ///
    /// For saying *why* a capture read nothing. Both numbers frozen means the backend is not on
    /// the keyboard; the first moving and the second not means the grab was not armed when the
    /// keys went by; both moving means the keys arrived and were not a binding.
    fn key_counts(&self) -> (u64, u64);
}

/// The presses this process has taken, so that their releases can be taken too.
///
/// A grab that swallows a key-down and then lets the key-up through leaves every application
/// behind it believing the key is still held: Ctrl sticks, and the next letter typed is a
/// shortcut instead of a letter. The reverse is just as bad -- taking a release whose press the
/// application *did* see leaves it holding a key nobody is pressing.
///
/// So the rule is exact and has no exceptions: **a release is taken if and only if its press was
/// taken.** That obligation is not part of the capture session and must outlive it, because the
/// session ends the instant a combination is read, while the user's fingers are still down.
#[derive(Clone, Copy, Debug, Default)]
pub struct SwallowLedger {
    keys: KeyBitmap,
}

impl SwallowLedger {
    /// A ledger owing nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that this key's press was taken.
    pub fn took_press(&mut self, code: u32) {
        self.keys.set(code, true);
    }

    /// Whether this key's release must be taken too. Clears the record either way, so one press
    /// buys exactly one release: a second key-up for the same key belongs to the application.
    pub fn owes_release(&mut self, code: u32) -> bool {
        let owed = self.keys.is_down(code);
        self.keys.set(code, false);
        owed
    }

    /// Whether any release is still owed.
    pub fn any_owed(&self) -> bool {
        self.keys.0.iter().any(|&w| w != 0)
    }
}

#[cfg(test)]
mod swallow_ledger_tests {
    use super::*;

    #[test]
    fn a_release_is_taken_exactly_when_its_press_was() {
        let mut led = SwallowLedger::new();
        assert!(!led.owes_release(0x41), "a release we never took a press for");
        led.took_press(0x41);
        assert!(led.owes_release(0x41));
        assert!(!led.owes_release(0x41), "one press buys one release, not two");
    }

    #[test]
    fn a_key_held_when_the_grab_opened_is_not_stolen_on_the_way_up() {
        // The stuck-key case, exactly. The application saw Ctrl go down before the editor took
        // the keyboard; if the editor then ate the key-up, that application would hold Ctrl for
        // ever and every subsequent letter would be a shortcut.
        let mut led = SwallowLedger::new();
        assert!(!led.owes_release(u32::from(vk::LCONTROL)));
    }

    #[test]
    fn the_ledger_tracks_each_key_on_its_own() {
        let mut led = SwallowLedger::new();
        led.took_press(u32::from(vk::LCONTROL));
        led.took_press(0x44);
        assert!(led.any_owed());
        assert!(led.owes_release(0x44));
        assert!(led.any_owed(), "Ctrl is still owed");
        assert!(led.owes_release(u32::from(vk::LCONTROL)));
        assert!(!led.any_owed());
    }

    #[test]
    fn keys_it_cannot_record_are_never_claimed() {
        let mut led = SwallowLedger::new();
        led.took_press(999);
        assert!(!led.any_owed());
        assert!(!led.owes_release(999));
    }
}

/// The pure half of a capture session: feed physical key transitions, get a [`Captured`] out.
///
/// Two ways a gesture ends, because there are two kinds of binding.
///
/// - A key ends it the moment it goes **down**. A bare modifier does not: the user is still on the
///   way to the real key, and holding Ctrl and then pressing D must bind Ctrl+D, not Ctrl.
/// - Modifiers alone end it when the last of them comes back **up**, and only if at least two were
///   held. That is the Ctrl+Meta push-to-talk the hook backend exists for, and waiting for the
///   release is what tells it apart from a user on their way to Ctrl+Meta+D. One modifier alone
///   is not offered: it would fire every time the user pressed Ctrl for any other reason.
#[derive(Clone, Copy, Debug, Default)]
pub struct CaptureTracker {
    keys: KeyBitmap,
    /// Every modifier family held at any point in the gesture so far.
    ///
    /// The *peak*, not what is down now, because the gesture is read when the keys come up: by the
    /// time the last one is released, none of them is still down to be counted.
    peak: u8,
}

impl CaptureTracker {
    /// A tracker with all keys up.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any key at all is held right now.
    ///
    /// The capture ends on a key-*down*, but the keys are still under the user's fingers at that
    /// moment. The grab has to outlive the gesture by exactly that much, or the releases of keys
    /// whose presses it swallowed would reach the window behind it -- a key-up with no key-down,
    /// which some applications read as a tap.
    pub fn any_down(&self) -> bool {
        self.keys.0.iter().any(|&w| w != 0)
    }

    /// Which modifier families are held right now.
    pub fn mods_down(&self) -> u8 {
        [MOD_CTRL, MOD_SHIFT, MOD_ALT, MOD_WIN]
            .into_iter()
            .filter(|&f| family_down(&self.keys, f))
            .fold(0, |acc, f| acc | f)
    }

    /// Process one key transition; returns a result on the transition that completes the gesture.
    pub fn on_key(&mut self, code: u32, down: bool) -> Option<Captured> {
        if code > 255 {
            // Same rule as [`KeyBitmap`], which cannot record these: a key the tracker could not
            // then see released must not start a gesture it could never end.
            return None;
        }
        self.keys.set(code, down);
        if !down {
            // A key going up ends the gesture only once nothing at all is held, and only for a
            // binding made of modifiers alone -- a key binding was settled on the way down.
            if self.any_down() {
                return None;
            }
            let peak = std::mem::take(&mut self.peak);
            return (peak.count_ones() >= 2)
                .then_some(Captured::Combo(RawCombo { mods: peak, vk: 0 }));
        }
        let vk = u16::try_from(code).ok()?;
        self.peak |= self.mods_down();
        if vk_family(vk).is_some() {
            // Still holding modifiers -- on the way to a real key, or to letting go.
            return None;
        }
        let mods = self.mods_down();
        // The gesture is settled here, so the modifier releases that follow belong to nothing.
        self.peak = 0;
        if vk == vk::ESCAPE && mods == 0 {
            return Some(Captured::Cancelled);
        }
        Some(Captured::Combo(RawCombo { mods, vk }))
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[test]
    fn the_windows_key_counts_as_a_modifier() {
        // The whole point of capturing through the keyboard hook rather than through the window:
        // Win+D never reaches a window, because the shell takes it first.
        let mut t = CaptureTracker::new();
        assert_eq!(t.on_key(u32::from(vk::LWIN), true), None, "Win alone waits");
        assert_eq!(
            t.on_key(0x44, true),
            Some(Captured::Combo(RawCombo { mods: MOD_WIN, vk: 0x44 })),
        );
    }

    #[test]
    fn every_held_family_is_reported() {
        let mut t = CaptureTracker::new();
        t.on_key(u32::from(vk::LCONTROL), true);
        t.on_key(u32::from(vk::RWIN), true);
        t.on_key(u32::from(vk::LSHIFT), true);
        assert_eq!(
            t.on_key(0x20, true),
            Some(Captured::Combo(RawCombo {
                mods: MOD_CTRL | MOD_WIN | MOD_SHIFT,
                vk: 0x20,
            })),
        );
    }

    #[test]
    fn a_released_modifier_is_no_longer_part_of_it() {
        let mut t = CaptureTracker::new();
        t.on_key(u32::from(vk::LCONTROL), true);
        t.on_key(u32::from(vk::LSHIFT), true);
        assert_eq!(t.on_key(u32::from(vk::LSHIFT), false), None);
        assert_eq!(
            t.on_key(0x20, true),
            Some(Captured::Combo(RawCombo { mods: MOD_CTRL, vk: 0x20 })),
        );
    }

    #[test]
    fn esc_on_its_own_cancels_but_esc_with_a_modifier_binds() {
        let mut t = CaptureTracker::new();
        assert_eq!(t.on_key(u32::from(vk::ESCAPE), true), Some(Captured::Cancelled));

        let mut t = CaptureTracker::new();
        t.on_key(u32::from(vk::LCONTROL), true);
        assert_eq!(
            t.on_key(u32::from(vk::ESCAPE), true),
            Some(Captured::Combo(RawCombo { mods: MOD_CTRL, vk: vk::ESCAPE })),
        );
    }

    #[test]
    fn two_modifiers_and_nothing_else_are_read_when_the_last_one_is_let_go() {
        // Ctrl+Meta push-to-talk, which is a binding the hook backend supports and which capture
        // previously could not produce at all -- it waited for a key that was never coming.
        let mut t = CaptureTracker::new();
        assert_eq!(t.on_key(u32::from(vk::LCONTROL), true), None);
        assert_eq!(t.on_key(u32::from(vk::LWIN), true), None);
        assert_eq!(t.on_key(u32::from(vk::LWIN), false), None, "one still held");
        assert_eq!(
            t.on_key(u32::from(vk::LCONTROL), false),
            Some(Captured::Combo(RawCombo {
                mods: MOD_CTRL | MOD_WIN,
                vk: 0,
            })),
        );
    }

    #[test]
    fn one_modifier_on_its_own_is_not_a_binding_and_the_capture_waits() {
        // Brushing Ctrl must not end the capture, and must not leave that Ctrl in whatever is
        // captured next.
        let mut t = CaptureTracker::new();
        assert_eq!(t.on_key(u32::from(vk::LCONTROL), true), None);
        assert_eq!(t.on_key(u32::from(vk::LCONTROL), false), None);
        assert_eq!(
            t.on_key(u32::from(vk::LSHIFT), true),
            None,
            "the tracker is still listening",
        );
        assert_eq!(
            t.on_key(0x44, true),
            Some(Captured::Combo(RawCombo { mods: MOD_SHIFT, vk: 0x44 })),
            "the abandoned Ctrl is not part of it",
        );
    }

    #[test]
    fn letting_go_after_a_key_has_been_read_produces_nothing_more() {
        // Ctrl+D is settled on D's way down. The releases that follow are the same gesture, and a
        // second event from them would rebind the shortcut to Ctrl+Shift by accident.
        let mut t = CaptureTracker::new();
        t.on_key(u32::from(vk::LCONTROL), true);
        t.on_key(u32::from(vk::LSHIFT), true);
        assert!(t.on_key(0x44, true).is_some());
        assert_eq!(t.on_key(0x44, false), None);
        assert_eq!(t.on_key(u32::from(vk::LSHIFT), false), None);
        assert_eq!(t.on_key(u32::from(vk::LCONTROL), false), None);
    }

    #[test]
    fn a_modifiers_only_capture_names_no_trigger_key() {
        assert_eq!(vk_name(0).as_deref(), Some("none"));
        assert_eq!(trigger_vk("none"), Some(0));
    }

    #[test]
    fn a_key_going_up_never_ends_the_capture() {
        let mut t = CaptureTracker::new();
        assert_eq!(t.on_key(0x44, false), None);
    }

    #[test]
    fn an_unbindable_key_still_arrives_so_the_editor_can_say_so() {
        // VK_VOLUME_UP: what a laptop sends for Fn+F3 on many keyboards. Reported, then refused
        // by name -- silence would look like a capture that simply does not work.
        let mut t = CaptureTracker::new();
        let out = t.on_key(0xAF, true);
        assert_eq!(out, Some(Captured::Combo(RawCombo { mods: 0, vk: 0xAF })));
        assert_eq!(vk_name(0xAF), None);
    }
}

#[cfg(test)]
mod vk_name_tests {
    use super::*;

    #[test]
    fn every_name_it_gives_round_trips_back_to_the_same_code() {
        for code in 0u16..=0xFF {
            let Some(name) = vk_name(code) else { continue };
            assert_eq!(trigger_vk(&name), Some(code), "{name} came from {code:#x}");
        }
    }

    #[test]
    fn the_keys_the_editor_offers_are_all_nameable() {
        let offered = ["space", "tab", "enter", "esc"]
            .into_iter()
            .map(String::from)
            .chain((b'a'..=b'z').map(|c| (c as char).to_string()))
            .chain((0..10).map(|d| d.to_string()))
            .chain((1..=20).map(|n| format!("f{n}")));
        for name in offered {
            let code = trigger_vk(&name).expect("the editor only offers parseable names");
            assert_eq!(vk_name(code).as_deref(), Some(name.as_str()));
        }
    }

    #[test]
    fn a_modifier_has_no_trigger_name() {
        for code in [vk::LWIN, vk::RWIN, vk::LCONTROL, vk::SHIFT, vk::LMENU] {
            assert_eq!(vk_name(code), None, "{code:#x}");
        }
    }
}

#[cfg(test)]
mod swallow_tests {
    use super::*;

    fn combo(mods: &[&str], trigger: &str) -> ParsedCombo {
        parse_spec(&HotkeySpec {
            modifiers: mods.iter().map(|s| s.to_string()).collect(),
            trigger: trigger.to_string(),
        })
        .expect("combo parses")
    }

    #[test]
    fn the_trigger_is_taken_while_the_combo_is_satisfied() {
        let c = combo(&["ctrl"], "space");
        let mut sw = false;
        assert!(swallow_trigger(&c, true, &mut sw, 0x20, true), "key-down");
        assert!(swallow_trigger(&c, false, &mut sw, 0x20, false), "key-up");
        assert!(!sw, "the flag must clear on the way up");
    }

    #[test]
    fn auto_repeat_is_taken_too() {
        // Windows sends key-down over and over while the keys are held. One escaping into the
        // focused window is one stray character in the middle of whatever the user was writing.
        let c = combo(&["ctrl"], "space");
        let mut sw = false;
        assert!(swallow_trigger(&c, true, &mut sw, 0x20, true));
        for _ in 0..5 {
            assert!(swallow_trigger(&c, true, &mut sw, 0x20, true), "repeat");
        }
        assert!(swallow_trigger(&c, false, &mut sw, 0x20, false));
    }

    #[test]
    fn the_same_key_without_the_modifiers_is_left_alone() {
        // Space is Space when Ctrl is not held, and a dictation app that ate every space would be
        // unusable.
        let c = combo(&["ctrl"], "space");
        let mut sw = false;
        assert!(!swallow_trigger(&c, false, &mut sw, 0x20, true));
        assert!(!swallow_trigger(&c, false, &mut sw, 0x20, false));
    }

    #[test]
    fn modifiers_are_never_taken() {
        let c = combo(&["ctrl"], "space");
        let mut sw = false;
        for vk in [vk::CONTROL, vk::LCONTROL, vk::RCONTROL, vk::SHIFT] {
            assert!(!swallow_trigger(&c, true, &mut sw, u32::from(vk), true), "{vk:#x}");
            assert!(!swallow_trigger(&c, true, &mut sw, u32::from(vk), false), "{vk:#x}");
        }
    }

    #[test]
    fn a_modifiers_only_combo_takes_nothing() {
        // There is no trigger to take, and taking Ctrl would break Ctrl+C everywhere.
        let c = combo(&["ctrl", "win"], "none");
        let mut sw = false;
        assert!(!swallow_trigger(&c, true, &mut sw, u32::from(vk::CONTROL), true));
        assert!(!swallow_trigger(&c, true, &mut sw, 0x20, true));
    }

    #[test]
    fn an_unrelated_key_is_left_alone() {
        let c = combo(&["ctrl"], "space");
        let mut sw = false;
        assert!(!swallow_trigger(&c, true, &mut sw, u32::from(b'D'), true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(mods: &[&str], trigger: &str) -> ParsedCombo {
        parse_spec(&HotkeySpec {
            modifiers: mods.iter().map(|s| s.to_string()).collect(),
            trigger: trigger.to_string(),
        })
        .unwrap()
    }

    #[test]
    fn parse_modifiers_and_trigger() {
        let c = combo(&["ctrl", "shift"], "space");
        assert_eq!(c.mods, MOD_CTRL | MOD_SHIFT);
        assert_eq!(c.trigger, 0x20);
    }

    #[test]
    fn parse_modifiers_only() {
        let c = combo(&["ctrl", "win"], "none");
        assert_eq!(c.mods, MOD_CTRL | MOD_WIN);
        assert_eq!(c.trigger, 0);
        let c2 = combo(&["ctrl", "win"], "");
        assert_eq!(c, c2);
    }

    #[test]
    fn parse_letters_digits_fkeys() {
        assert_eq!(trigger_vk("v"), Some(0x56));
        assert_eq!(trigger_vk("A"), Some(0x41));
        assert_eq!(trigger_vk("7"), Some(0x37));
        assert_eq!(trigger_vk("f1"), Some(0x70));
        assert_eq!(trigger_vk("F13"), Some(0x7C));
        assert_eq!(trigger_vk("f24"), Some(0x87));
        assert_eq!(trigger_vk("f25"), None);
        assert_eq!(trigger_vk("bogus"), None);
    }

    #[test]
    fn parse_rejects_unknowns_and_empty() {
        assert!(
            parse_spec(&HotkeySpec {
                modifiers: vec!["hyper".into()],
                trigger: "space".into()
            })
            .is_err()
        );
        assert!(
            parse_spec(&HotkeySpec {
                modifiers: vec![],
                trigger: "bogus".into()
            })
            .is_err()
        );
        assert!(
            parse_spec(&HotkeySpec {
                modifiers: vec![],
                trigger: "none".into()
            })
            .is_err()
        );
    }

    #[test]
    fn modifiers_only_combo_edges() {
        // Ctrl+Win push-to-talk
        let mut t = ComboTracker::new(combo(&["ctrl", "win"], "none"));
        assert_eq!(t.on_key(vk::LCONTROL as u32, true), None); // ctrl alone: nothing
        assert_eq!(t.on_key(vk::LWIN as u32, true), Some(HotkeyEvent::Pressed));
        assert!(t.is_active());
        // holding — key repeats change nothing
        assert_eq!(t.on_key(vk::LCONTROL as u32, true), None);
        assert_eq!(t.on_key(vk::LWIN as u32, false), Some(HotkeyEvent::Released));
        assert_eq!(t.on_key(vk::LCONTROL as u32, false), None);
    }

    #[test]
    fn right_side_modifiers_count() {
        let mut t = ComboTracker::new(combo(&["ctrl", "win"], "none"));
        assert_eq!(t.on_key(vk::RCONTROL as u32, true), None);
        assert_eq!(t.on_key(vk::RWIN as u32, true), Some(HotkeyEvent::Pressed));
        assert_eq!(t.on_key(vk::RCONTROL as u32, false), Some(HotkeyEvent::Released));
    }

    #[test]
    fn trigger_combo_edges() {
        // Ctrl+Space
        let mut t = ComboTracker::new(combo(&["ctrl"], "space"));
        assert_eq!(t.on_key(vk::LCONTROL as u32, true), None);
        assert_eq!(t.on_key(0x20, true), Some(HotkeyEvent::Pressed));
        assert_eq!(t.on_key(0x20, false), Some(HotkeyEvent::Released));
        // pressing space again while ctrl still held re-fires
        assert_eq!(t.on_key(0x20, true), Some(HotkeyEvent::Pressed));
        assert_eq!(t.on_key(vk::LCONTROL as u32, false), Some(HotkeyEvent::Released));
    }

    #[test]
    fn unconfigured_modifier_blocks_and_releases() {
        // Strict matching: Ctrl+Shift+Space must not fire a Ctrl+Space combo,
        // and adding Shift mid-hold releases it.
        let mut t = ComboTracker::new(combo(&["ctrl"], "space"));
        t.on_key(vk::LCONTROL as u32, true);
        assert_eq!(t.on_key(vk::LSHIFT as u32, true), None);
        assert_eq!(t.on_key(0x20, true), None); // blocked by shift
        assert_eq!(t.on_key(vk::LSHIFT as u32, false), Some(HotkeyEvent::Pressed));
        assert_eq!(t.on_key(vk::LSHIFT as u32, true), Some(HotkeyEvent::Released));
    }

    #[test]
    fn trigger_in_modifier_family_is_allowed() {
        // A bare right_ctrl trigger: the ctrl family is down because of the trigger
        // itself; that must not block activation.
        let mut t = ComboTracker::new(combo(&[], "right_ctrl"));
        assert_eq!(t.on_key(vk::RCONTROL as u32, true), Some(HotkeyEvent::Pressed));
        assert_eq!(t.on_key(vk::RCONTROL as u32, false), Some(HotkeyEvent::Released));
    }

    #[test]
    fn order_of_presses_does_not_matter() {
        let mut t = ComboTracker::new(combo(&["ctrl", "alt"], "d"));
        t.on_key(0x44, true); // d first
        t.on_key(vk::LMENU as u32, true);
        assert_eq!(t.on_key(vk::LCONTROL as u32, true), Some(HotkeyEvent::Pressed));
    }

    #[test]
    fn reset_releases_if_held() {
        let mut t = ComboTracker::new(combo(&["ctrl", "win"], "none"));
        t.on_key(vk::LCONTROL as u32, true);
        t.on_key(vk::LWIN as u32, true);
        assert_eq!(t.reset(), Some(HotkeyEvent::Released));
        assert_eq!(t.reset(), None);
        assert!(!t.is_active());
    }

    #[test]
    fn out_of_range_codes_are_ignored() {
        let mut bits = KeyBitmap::default();
        bits.set(999, true);
        assert!(!bits.is_down(999));
        let mut t = ComboTracker::new(combo(&["ctrl"], "none"));
        assert_eq!(t.on_key(999, true), None);
    }

    #[test]
    fn generic_modifier_codes_count_for_family() {
        let mut bits = KeyBitmap::default();
        bits.set(vk::CONTROL as u32, true);
        assert!(family_down(&bits, MOD_CTRL));
        assert!(!family_down(&bits, MOD_SHIFT));
    }
}
