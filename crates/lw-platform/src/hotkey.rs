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
