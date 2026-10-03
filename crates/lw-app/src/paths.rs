//! Where this application keeps its things.
//!
//! This matters more than it looks. The Tauri build resolved its data directory through
//! `AppHandle::path().app_data_dir()`, which on Windows is `%APPDATA%/<bundle identifier>` -- and
//! the bundle identifier lives in `tauri.conf.json`, a file the native front end does not read and
//! will not ship. A native window that computed its own directory instead would look at an empty
//! folder and report that none of the user's eight installed models exist, several gigabytes of
//! them, with nothing on screen to suggest it was looking in the wrong place.
//!
//! So the identifier is written here, once, and both front ends use it. `lw_core`'s
//! `default_models_root` is a different thing and deliberately stays different: it is where the
//! CLI looks when nobody told it otherwise, and `LW_MODELS_ROOT` is how the CLI is pointed at the
//! app's directory instead.

use std::path::PathBuf;

/// The bundle identifier, matching `app/src-tauri/tauri.conf.json`.
///
/// Changing this orphans every installed model and every saved setting of every existing user.
pub const APP_IDENTIFIER: &str = "ai.owlwhisp.app";

/// The directory holding `settings.json`, `measurements.json` and `models/`.
///
/// `%APPDATA%/ai.owlwhisp.app` on Windows, `~/.local/share/ai.owlwhisp.app` elsewhere --
/// the same places Tauri's `app_data_dir()` resolves to, because this has to find what that wrote.
pub fn app_data_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("LW_APP_DATA_DIR")
        && !explicit.trim().is_empty()
    {
        return PathBuf::from(explicit);
    }
    #[cfg(windows)]
    if let Ok(base) = std::env::var("APPDATA") {
        return PathBuf::from(base).join(APP_IDENTIFIER);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join(APP_IDENTIFIER);
    }
    PathBuf::from(APP_IDENTIFIER)
}

/// The settings file both front ends read and write.
pub fn settings_path() -> PathBuf {
    app_data_dir().join("settings.json")
}

/// Where models are installed.
///
/// `LW_MODELS_ROOT` still wins, so a developer can point a build at a scratch directory without
/// moving the real one.
pub fn models_root() -> PathBuf {
    if let Ok(explicit) = std::env::var("LW_MODELS_ROOT")
        && !explicit.trim().is_empty()
    {
        return PathBuf::from(explicit);
    }
    app_data_dir().join("models")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identifier_is_the_one_every_existing_installation_already_uses() {
        // Pinned to a literal on purpose, and not derived from anything. This string is the name
        // of the folder holding every installed model, the settings file and the accumulated
        // measurements on every machine the application has ever run on. Changing it does not
        // migrate them; it hides them, and the user sees an empty model list and a fresh
        // settings file with no explanation. It was checked against the Tauri bundle identifier
        // until that build was deleted; the value has to outlive the thing it was copied from.
        assert_eq!(APP_IDENTIFIER, "ai.owlwhisp.app");
    }

    #[test]
    fn the_models_root_sits_under_the_data_directory() {
        // Guards the thing that would silently empty the model list: a root computed from a
        // different base than the settings file it is supposed to sit beside.
        unsafe {
            std::env::remove_var("LW_MODELS_ROOT");
        }
        let root = models_root();
        let data = app_data_dir();
        assert!(
            root.starts_with(&data),
            "{} is not under {}",
            root.display(),
            data.display()
        );
        assert_eq!(settings_path().parent(), Some(data.as_path()));
    }
}
