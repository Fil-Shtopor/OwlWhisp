//! Per-application profiles.
//!
//! A profile bundles the behaviour that should apply while a particular app is in the foreground:
//! which dictionary, cleanup level, language, and backend to use, and how to deliver text. The
//! matcher resolves the active profile from the foreground application's executable/bundle id and
//! window title. There is always a `default` profile; an explicit match beats it, and a manual
//! override beats the auto-match for the session.

use serde::{Deserialize, Serialize};

use crate::engine::BackendPreference;

/// How aggressively to clean up transcribed text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupLevel {
    /// No cleanup beyond the model's own output.
    None,
    /// Deterministic cleanup only (whitespace, spoken punctuation, casing).
    #[default]
    Light,
    /// Deterministic + optional LLM polish (filler removal, formatting).
    Full,
}

/// Text-delivery options for a profile.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DeliveryOptions {
    /// Paste automatically after transcription (vs. copy to clipboard only).
    pub auto_paste: bool,
    /// Restore the previous clipboard contents after pasting.
    pub restore_clipboard: bool,
    /// Remove a single trailing period (chat apps).
    pub strip_trailing_period: bool,
    /// Press Enter after pasting.
    pub press_enter: bool,
}

impl Default for DeliveryOptions {
    fn default() -> Self {
        Self {
            auto_paste: true,
            restore_clipboard: true,
            strip_trailing_period: false,
            press_enter: false,
        }
    }
}

/// A rule for auto-activating a profile.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MatchRule {
    /// Match if the foreground executable/bundle id contains this (case-insensitive).
    #[serde(default)]
    pub exe_contains: Vec<String>,
    /// Match if the window title matches this substring (case-insensitive).
    #[serde(default)]
    pub title_contains: Vec<String>,
}

/// A single application profile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    /// Stable key (used for deep links / storage).
    pub key: String,
    /// Display name.
    pub name: String,
    /// Language hint (`"auto"` or a code).
    #[serde(default = "default_language")]
    pub language: String,
    /// Cleanup level.
    #[serde(default)]
    pub cleanup: CleanupLevel,
    /// Backend preference override.
    #[serde(default)]
    pub backend: BackendPreference,
    /// Delivery options.
    #[serde(default)]
    pub delivery: DeliveryOptions,
    /// Auto-activation rules.
    #[serde(default)]
    pub match_rule: MatchRule,
}

fn default_language() -> String {
    "auto".to_string()
}

impl Profile {
    /// The built-in default profile.
    pub fn default_profile() -> Self {
        Self {
            key: "default".into(),
            name: "Default".into(),
            language: default_language(),
            cleanup: CleanupLevel::Light,
            backend: BackendPreference::Automatic,
            delivery: DeliveryOptions::default(),
            match_rule: MatchRule::default(),
        }
    }

    /// A code/terminal profile: no cleanup, Ctrl+Shift+V-style paste handled by injector,
    /// no trailing-period stripping, no Enter.
    pub fn code_profile() -> Self {
        Self {
            key: "code".into(),
            name: "Code / Terminal".into(),
            language: default_language(),
            cleanup: CleanupLevel::None,
            backend: BackendPreference::Automatic,
            delivery: DeliveryOptions {
                press_enter: false,
                strip_trailing_period: false,
                ..Default::default()
            },
            match_rule: MatchRule {
                exe_contains: vec![
                    "code".into(),
                    "windowsterminal".into(),
                    "wt".into(),
                    "conhost".into(),
                    "powershell".into(),
                    "cmd".into(),
                    "alacritty".into(),
                    "wezterm".into(),
                    "iterm".into(),
                    "terminal".into(),
                    "idea".into(),
                    "pycharm".into(),
                    "clion".into(),
                    "goland".into(),
                ],
                title_contains: vec![],
            },
        }
    }

    /// A messaging profile: casual, strips trailing period.
    pub fn message_profile() -> Self {
        Self {
            key: "message".into(),
            name: "Message".into(),
            language: default_language(),
            cleanup: CleanupLevel::Light,
            backend: BackendPreference::Automatic,
            delivery: DeliveryOptions {
                strip_trailing_period: true,
                ..Default::default()
            },
            match_rule: MatchRule {
                exe_contains: vec![
                    "slack".into(),
                    "discord".into(),
                    "teams".into(),
                    "telegram".into(),
                    "whatsapp".into(),
                ],
                title_contains: vec![],
            },
        }
    }
}

/// Foreground application context used to resolve a profile.
#[derive(Clone, Debug, Default)]
pub struct ForegroundApp {
    /// Executable path or bundle id.
    pub exe: String,
    /// Window title.
    pub title: String,
}

/// A set of profiles with a matcher.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileSet {
    /// All profiles; the one keyed `"default"` is the fallback.
    pub profiles: Vec<Profile>,
}

impl Default for ProfileSet {
    fn default() -> Self {
        Self {
            profiles: vec![
                Profile::default_profile(),
                Profile::code_profile(),
                Profile::message_profile(),
            ],
        }
    }
}

impl ProfileSet {
    /// Find a profile by key.
    pub fn by_key(&self, key: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.key == key)
    }

    /// The default profile (always present; falls back to a synthesized default).
    pub fn default_profile(&self) -> Profile {
        self.by_key("default")
            .cloned()
            .unwrap_or_else(Profile::default_profile)
    }

    /// Resolve the active profile for a foreground app. Non-default profiles are matched first;
    /// the default is the fallback. A title match or an exe match counts.
    pub fn resolve(&self, app: &ForegroundApp) -> Profile {
        let exe = app.exe.to_lowercase();
        let title = app.title.to_lowercase();
        for p in self.profiles.iter().filter(|p| p.key != "default") {
            let exe_hit = p
                .match_rule
                .exe_contains
                .iter()
                .any(|c| !c.is_empty() && exe.contains(&c.to_lowercase()));
            let title_hit = p
                .match_rule
                .title_contains
                .iter()
                .any(|c| !c.is_empty() && title.contains(&c.to_lowercase()));
            if exe_hit || title_hit {
                return p.clone();
            }
        }
        self.default_profile()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_code_profile_for_vscode() {
        let set = ProfileSet::default();
        let app = ForegroundApp {
            exe: "C:/Users/x/AppData/Local/Programs/Microsoft VS Code/Code.exe".into(),
            title: "main.rs".into(),
        };
        assert_eq!(set.resolve(&app).key, "code");
    }

    #[test]
    fn resolves_message_profile_for_slack() {
        let set = ProfileSet::default();
        let app = ForegroundApp {
            exe: "/Applications/Slack.app".into(),
            title: "general".into(),
        };
        let p = set.resolve(&app);
        assert_eq!(p.key, "message");
        assert!(p.delivery.strip_trailing_period);
    }

    #[test]
    fn falls_back_to_default() {
        let set = ProfileSet::default();
        let app = ForegroundApp {
            exe: "notepad.exe".into(),
            title: "Untitled".into(),
        };
        assert_eq!(set.resolve(&app).key, "default");
    }

    #[test]
    fn serializes() {
        let set = ProfileSet::default();
        let json = serde_json::to_string(&set).unwrap();
        let back: ProfileSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.profiles.len(), set.profiles.len());
    }
}
