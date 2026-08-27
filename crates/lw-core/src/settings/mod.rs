//! Application settings: a typed, versioned struct with atomic load/save and a schema.
//!
//! Settings persist as pretty JSON in the OS app-data dir. Saving is atomic (write temp + rename)
//! so a crash mid-write never corrupts the file. Unknown fields are preserved on the struct's
//! `extra` map for forward compatibility.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::dictionary::Dictionary;
use crate::engine::BackendPreference;
use crate::profiles::ProfileSet;
use crate::vad::EndpointConfig;
use crate::{Error, Result};

/// How the push-to-talk hotkey behaves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyMode {
    /// Hold to talk, release to stop.
    #[default]
    PushToTalk,
    /// Tap to start, tap to stop.
    Toggle,
    /// Tap to start; VAD ends the utterance.
    HandsFree,
}

/// Hotkey configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HotkeyConfig {
    /// Modifier list, e.g. `["ctrl", "win"]`.
    pub modifiers: Vec<String>,
    /// Trigger key, or `"none"` for modifiers-only.
    pub trigger: String,
    /// PTT / toggle / hands-free.
    pub mode: HotkeyMode,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            modifiers: vec!["ctrl".into(), "alt".into()],
            trigger: "space".into(),
            mode: HotkeyMode::PushToTalk,
        }
    }
}

impl HotkeyConfig {
    /// Render the binding as an accelerator string (`"Control+Alt+Space"`) for the OS shortcut
    /// registry.
    ///
    /// Returns `None` for a modifiers-only binding (`trigger == "none"`): the OS-level
    /// `RegisterHotKey` path needs a non-modifier key, so those bindings require the low-level
    /// keyboard hook backend in `lw-platform` instead.
    pub fn to_accelerator(&self) -> Option<String> {
        let mut parts: Vec<&'static str> = Vec::new();
        for m in &self.modifiers {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => parts.push("Control"),
                "shift" => parts.push("Shift"),
                "alt" | "option" => parts.push("Alt"),
                "win" | "super" | "cmd" | "meta" => parts.push("Super"),
                _ => return None,
            }
        }
        let key = key_code_name(&self.trigger)?;
        parts.push(key);
        Some(parts.join("+"))
    }

    /// Whether this binding needs the low-level hook backend (modifiers-only combos).
    pub fn is_modifier_only(&self) -> bool {
        self.trigger.eq_ignore_ascii_case("none") || self.trigger.is_empty()
    }

    /// Validate the binding the way the shortcut editor should: a usable combination needs either a
    /// trigger key, or (for the hook backend) at least two modifiers.
    pub fn validate(&self) -> Result<()> {
        if self.is_modifier_only() {
            // Accepting this would be a lie: an OS global-shortcut registration needs a
            // non-modifier key, so the app would silently fall back to the default binding and
            // the user's chosen keys would never fire. Reject it here, where the message can
            // reach the settings UI, rather than warning into a log nobody reads. Supporting it
            // needs a low-level keyboard hook backend, which this build does not ship.
            return Err(Error::Config(
                "a hotkey needs a non-modifier key (modifiers-only bindings need a low-level                  keyboard hook, which this build does not provide)"
                    .into(),
            ));
        }
        if key_code_name(&self.trigger).is_none() {
            return Err(Error::Config(format!("unsupported hotkey key: {}", self.trigger)));
        }
        Ok(())
    }
}

/// Map a trigger name to the `KeyboardEvent.code`-style name accelerators use.
fn key_code_name(trigger: &str) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK", "KeyL",
        "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV", "KeyW", "KeyX",
        "KeyY", "KeyZ",
    ];
    const DIGITS: [&str; 10] = [
        "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9",
    ];
    const FKEYS: [&str; 20] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15",
        "F16", "F17", "F18", "F19", "F20",
    ];
    let t = trigger.trim().to_ascii_lowercase();
    match t.as_str() {
        "" | "none" => return None,
        "space" => return Some("Space"),
        "tab" => return Some("Tab"),
        "enter" | "return" => return Some("Enter"),
        "escape" | "esc" => return Some("Escape"),
        "capslock" | "caps_lock" => return Some("CapsLock"),
        "insert" => return Some("Insert"),
        "backquote" | "grave" => return Some("Backquote"),
        _ => {}
    }
    if t.len() == 1 {
        let c = t.as_bytes()[0];
        if c.is_ascii_lowercase() {
            return Some(LETTERS[(c - b'a') as usize]);
        }
        if c.is_ascii_digit() {
            return Some(DIGITS[(c - b'0') as usize]);
        }
    }
    if let Some(rest) = t.strip_prefix('f')
        && let Ok(n) = rest.parse::<usize>()
        && (1..=20).contains(&n)
    {
        return Some(FKEYS[n - 1]);
    }
    None
}

/// Optional OpenAI-compatible LLM cleanup endpoint.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Whether LLM cleanup is enabled at all.
    pub enabled: bool,
    /// Base URL, e.g. `https://api.openai.com/v1`.
    pub base_url: String,
    /// Model id.
    pub model: String,
    /// Whether the API key is stored in the OS keychain (the key itself is never in settings).
    pub key_in_keychain: bool,
}

/// Audio input settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Preferred input device name, or empty for the system default.
    pub input_device: String,
    /// Minimum recording length to accept (seconds).
    pub min_record_secs: f32,
    /// Maximum recording length (seconds).
    pub max_record_secs: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: String::new(),
            min_record_secs: 0.25,
            max_record_secs: 300.0,
        }
    }
}

/// The whole settings document. Bump `version` on breaking changes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// Schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Hotkey.
    pub hotkey: HotkeyConfig,
    /// Audio.
    pub audio: AudioConfig,
    /// Backend preference (Automatic/ForceNpu/ForceCpu).
    pub backend: BackendPreference,
    /// Active model id.
    pub model_id: String,
    /// VAD endpoint tuning.
    pub vad: EndpointConfig,
    /// Whether the overlay is shown.
    pub overlay_enabled: bool,
    /// Whether start/stop sounds play.
    pub sounds_enabled: bool,
    /// LLM cleanup config.
    pub llm: LlmConfig,
    /// Replacement dictionary.
    pub dictionary: Dictionary,
    /// Per-application profiles.
    pub profiles: ProfileSet,
    /// Log level: "error"|"warn"|"info"|"debug"|"trace".
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Unknown fields preserved for forward compatibility.
    #[serde(flatten, default)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

fn default_version() -> u32 {
    1
}
fn default_log_level() -> String {
    "info".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: default_version(),
            hotkey: HotkeyConfig::default(),
            audio: AudioConfig::default(),
            backend: BackendPreference::Automatic,
            model_id: "parakeet-tdt-0.6b-v3".into(),
            vad: EndpointConfig::default(),
            overlay_enabled: true,
            sounds_enabled: true,
            llm: LlmConfig::default(),
            dictionary: Dictionary::new(),
            profiles: ProfileSet::default(),
            log_level: default_log_level(),
            extra: Default::default(),
        }
    }
}

impl Settings {
    /// Validate ranges, returning a descriptive error if something is out of bounds.
    pub fn validate(&self) -> Result<()> {
        if self.audio.min_record_secs < 0.0 {
            return Err(Error::Config("audio.min_record_secs must be >= 0".into()));
        }
        if self.audio.max_record_secs <= 0.0 || self.audio.max_record_secs > 3600.0 {
            return Err(Error::Config("audio.max_record_secs must be in (0, 3600]".into()));
        }
        if !(0.0..=1.0).contains(&self.vad.threshold) {
            return Err(Error::Config("vad.threshold must be in [0, 1]".into()));
        }
        self.hotkey.validate()?;
        if self.model_id.trim().is_empty() {
            return Err(Error::Config("model_id must not be empty".into()));
        }
        Ok(())
    }

    /// Load from a JSON file, falling back to defaults if it does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let s: Settings = serde_json::from_slice(&bytes)?;
                s.validate()?;
                Ok(s)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(Error::io(path.display().to_string(), e)),
        }
    }

    /// Save atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent.display().to_string(), e))?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| Error::io(tmp.display().to_string(), e))?;
        std::fs::rename(&tmp, path).map_err(|e| Error::io(path.display().to_string(), e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accelerator_from_default_binding() {
        let h = HotkeyConfig::default();
        assert_eq!(h.to_accelerator().as_deref(), Some("Control+Alt+Space"));
        assert!(!h.is_modifier_only());
        h.validate().unwrap();
    }

    #[test]
    fn accelerator_maps_letters_digits_and_fkeys() {
        let mk = |trigger: &str| HotkeyConfig {
            modifiers: vec!["ctrl".into()],
            trigger: trigger.into(),
            mode: HotkeyMode::Toggle,
        };
        assert_eq!(mk("d").to_accelerator().as_deref(), Some("Control+KeyD"));
        assert_eq!(mk("F13").to_accelerator().as_deref(), Some("Control+F13"));
        assert_eq!(mk("1").to_accelerator().as_deref(), Some("Control+Digit1"));
        assert_eq!(mk("tab").to_accelerator().as_deref(), Some("Control+Tab"));
    }

    #[test]
    fn modifier_only_binding_has_no_accelerator() {
        let h = HotkeyConfig {
            modifiers: vec!["ctrl".into(), "win".into()],
            trigger: "none".into(),
            mode: HotkeyMode::PushToTalk,
        };
        assert!(h.is_modifier_only());
        assert_eq!(h.to_accelerator(), None);
    }

    #[test]
    fn modifier_only_bindings_are_rejected_because_they_would_silently_fall_back() {
        // No number of modifiers makes this registrable without a low-level hook backend, and
        // accepting it would leave the app listening on the default binding instead.
        for mods in [vec!["ctrl"], vec!["ctrl", "win"], vec!["ctrl", "alt", "shift"]] {
            let h = HotkeyConfig {
                modifiers: mods.iter().map(|m| m.to_string()).collect(),
                trigger: "none".into(),
                mode: HotkeyMode::PushToTalk,
            };
            let err = h.validate().unwrap_err().to_string();
            assert!(err.contains("non-modifier key"), "{err}");
        }
    }

    #[test]
    fn an_empty_trigger_is_rejected_too() {
        let h = HotkeyConfig {
            modifiers: vec!["ctrl".into(), "alt".into()],
            trigger: String::new(),
            mode: HotkeyMode::PushToTalk,
        };
        assert!(h.validate().is_err());
    }

    #[test]
    fn unknown_key_is_rejected() {
        let h = HotkeyConfig {
            modifiers: vec!["ctrl".into()],
            trigger: "wingding".into(),
            mode: HotkeyMode::Toggle,
        };
        assert!(h.validate().is_err());
        assert_eq!(h.to_accelerator(), None);
    }

    #[test]
    fn hotkey_modes_roundtrip_json() {
        for mode in [HotkeyMode::PushToTalk, HotkeyMode::Toggle, HotkeyMode::HandsFree] {
            let json = serde_json::to_string(&mode).unwrap();
            let back: HotkeyMode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, mode);
        }
        assert_eq!(serde_json::to_string(&HotkeyMode::Toggle).unwrap(), "\"toggle\"");
    }

    #[test]
    fn default_is_valid() {
        Settings::default().validate().unwrap();
    }

    #[test]
    fn rejects_bad_max_record() {
        let mut s = Settings::default();
        s.audio.max_record_secs = 0.0;
        assert!(s.validate().is_err());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut s = Settings::default();
        s.dictionary.add_exact("open wiser", "OpenWritr");
        s.save(&path).unwrap();
        let back = Settings::load(&path).unwrap();
        assert_eq!(back.dictionary.rules.len(), 1);
        assert_eq!(back.model_id, s.model_id);
    }

    #[test]
    fn missing_file_yields_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let s = Settings::load(&path).unwrap();
        assert_eq!(s.version, 1);
    }

    #[test]
    fn preserves_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let json = r#"{"version":1,"hotkey":{"modifiers":["ctrl","alt"],"trigger":"space","mode":"toggle"},
            "audio":{"input_device":"","min_record_secs":0.25,"max_record_secs":300.0},
            "backend":"automatic","model_id":"m","vad":{"threshold":0.5,"frame_ms":32.0,
            "min_speech_ms":200,"hangover_ms":1000,"pre_roll_ms":300,"trailing_pad_ms":250,"max_segment_ms":20000},
            "overlay_enabled":true,"sounds_enabled":true,
            "llm":{"enabled":false,"base_url":"","model":"","key_in_keychain":false},
            "dictionary":{"rules":[]},"profiles":{"profiles":[]},"log_level":"info",
            "future_field":42}"#;
        std::fs::write(&path, json).unwrap();
        let s = Settings::load(&path).unwrap();
        assert!(s.extra.contains_key("future_field"));
    }
}
