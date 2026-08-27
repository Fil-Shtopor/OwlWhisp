//! Where models live on this machine, and whether they are actually there.
//!
//! Both the CLI (`lw models ...`) and the desktop app need to answer the same three questions --
//! *where is the models root*, *where are the pinned manifests*, and *is this entry installed* --
//! and they must answer them identically, or the app would offer to download something the CLI
//! already installed. So the logic lives here once rather than in each front end.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::capabilities::Capabilities;
use crate::model::catalog::CatalogEntry;
use crate::model::manifest::ArtifactTarget;
use crate::model::{CacheState, ModelManifest, ModelRegistry};

/// The per-user models directory used when the caller does not pass one.
///
/// `$LW_MODELS_ROOT` wins when set. Otherwise Windows uses `%LOCALAPPDATA%\LocalWisper\models`
/// and Unix uses `$HOME/.local/share/LocalWisper/models`, falling back to a relative `models`
/// directory when neither variable is set.
///
/// The desktop app does **not** use this: it keeps models under its own Tauri app-data directory
/// and passes that root explicitly. `LW_MODELS_ROOT` is how you point the CLI at the app's copy
/// so `lw models list` and the app agree about what is installed.
pub fn default_models_root() -> PathBuf {
    if let Ok(explicit) = std::env::var("LW_MODELS_ROOT")
        && !explicit.trim().is_empty()
    {
        return PathBuf::from(explicit);
    }
    if cfg!(windows)
        && let Ok(base) = std::env::var("LOCALAPPDATA")
    {
        return PathBuf::from(base).join("LocalWisper").join("models");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("LocalWisper")
            .join("models");
    }
    PathBuf::from("models")
}

/// Walk up from `start` looking for `rel`, returning the resolved path.
pub fn find_upwards(start: &Path, rel: &str) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join(rel);
        if candidate.exists() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Locate a repo-relative directory from the working directory or next to the executable.
///
/// This is how a dev build finds `models/manifests` without configuration. An installed build
/// ships the manifests next to the executable, which the second branch covers.
pub fn locate_repo_path(rel: &str) -> Option<PathBuf> {
    if let Ok(cwd) = std::env::current_dir()
        && let Some(p) = find_upwards(&cwd, rel)
    {
        return Some(p);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && let Some(p) = find_upwards(dir, rel)
    {
        return Some(p);
    }
    None
}

/// The directory holding pinned model manifests, or `None` when it cannot be found.
pub fn manifests_dir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    explicit.or_else(|| locate_repo_path("models/manifests"))
}

/// How a catalog entry stands on this disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    /// A pinned manifest exists and every file is present with the expected size.
    Installed,
    /// A pinned manifest exists and some files are missing or truncated.
    Incomplete,
    /// A pinned manifest exists and nothing is there.
    Missing,
    /// No pinned manifest, so there is nothing authoritative to check against.
    Unpinned,
}

impl InstallState {
    /// Short label for tabular output (`yes` / `partial` / `no` / `n/a`).
    pub fn label(self) -> &'static str {
        match self {
            InstallState::Installed => "yes",
            InstallState::Incomplete => "partial",
            InstallState::Missing => "no",
            InstallState::Unpinned => "n/a",
        }
    }
}

/// Everything path-related about one entry on this machine.
#[derive(Clone, Debug, Serialize)]
pub struct EntryPaths {
    /// Whether the pinned files are on disk.
    pub state: InstallState,
    /// Where the model would live (or does live).
    pub dir: Option<PathBuf>,
    /// Why the manifest could not be loaded, if it could not.
    pub manifest_error: Option<String>,
}

/// Artifact targets to prefer when picking files, best first, for this machine.
///
/// A QNN artifact is only preferred when the NPU is present **and** the QNN provider actually
/// loaded -- a driver package with no working EP must not make us download an NPU-only artifact.
pub fn preferred_targets(caps: &Capabilities) -> Vec<ArtifactTarget> {
    let mut prefs = Vec::new();
    if caps.npu.present && caps.providers.qnn {
        match caps.npu.htp_arch.map(|a| a.num()) {
            Some(81) => prefs.push(ArtifactTarget::QnnHtpV81),
            Some(73) => prefs.push(ArtifactTarget::QnnHtpV73),
            _ => {
                prefs.push(ArtifactTarget::QnnHtpV81);
                prefs.push(ArtifactTarget::QnnHtpV73);
            }
        }
    }
    if caps.providers.coreml {
        prefs.push(ArtifactTarget::CoreMl);
    }
    prefs.push(ArtifactTarget::CpuInt8);
    prefs.push(ArtifactTarget::Any);
    prefs
}

/// Resolve manifest + install state for an entry.
pub fn entry_paths(
    entry: &CatalogEntry,
    manifests: Option<&Path>,
    root: &Path,
    caps: &Capabilities,
) -> EntryPaths {
    let Some(name) = &entry.manifest else {
        return EntryPaths {
            state: InstallState::Unpinned,
            dir: None,
            manifest_error: None,
        };
    };
    let Some(mdir) = manifests else {
        return EntryPaths {
            state: InstallState::Unpinned,
            dir: None,
            manifest_error: Some("manifest directory not found".to_string()),
        };
    };
    let path = mdir.join(name);
    let manifest = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str::<ModelManifest>(&t).map_err(|e| e.to_string()))
        .and_then(|m| m.validate().map(|()| m).map_err(|e| e.to_string()));
    match manifest {
        Err(e) => EntryPaths {
            state: InstallState::Unpinned,
            dir: None,
            manifest_error: Some(format!("{}: {e}", path.display())),
        },
        Ok(m) => {
            let registry = ModelRegistry::new(root);
            let dir = registry.model_dir(&m);
            let state = match m.select_files(&preferred_targets(caps)) {
                Some((_, files)) => match registry.state(&m, &files) {
                    CacheState::Installed => InstallState::Installed,
                    CacheState::Incomplete => InstallState::Incomplete,
                    CacheState::Missing => InstallState::Missing,
                },
                None => InstallState::Unpinned,
            };
            EntryPaths {
                state,
                dir: Some(dir),
                manifest_error: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_root_ends_in_models() {
        let p = default_models_root();
        assert_eq!(p.file_name().unwrap(), "models");
    }

    #[test]
    fn find_upwards_locates_a_parent_entry() {
        let base = std::env::temp_dir().join("lw-paths-test-up");
        let deep = base.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        let marker = base.join("marker.txt");
        std::fs::write(&marker, "x").unwrap();
        let found = find_upwards(&deep, "marker.txt");
        assert_eq!(found.as_deref(), Some(marker.as_path()));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn find_upwards_returns_none_when_absent() {
        let tmp = std::env::temp_dir();
        assert!(find_upwards(&tmp, "definitely-not-here-9f3a2b.txt").is_none());
    }

    #[test]
    fn manifests_dir_prefers_the_explicit_path() {
        let explicit = PathBuf::from("/somewhere/manifests");
        assert_eq!(manifests_dir(Some(explicit.clone())), Some(explicit));
    }

    #[test]
    fn install_state_labels_are_stable() {
        assert_eq!(InstallState::Installed.label(), "yes");
        assert_eq!(InstallState::Incomplete.label(), "partial");
        assert_eq!(InstallState::Missing.label(), "no");
        assert_eq!(InstallState::Unpinned.label(), "n/a");
    }

    #[test]
    fn preferred_targets_never_prefers_qnn_without_a_loaded_provider() {
        let mut caps = Capabilities::unknown();
        caps.npu.present = true;
        caps.providers.qnn = false; // driver present, EP did not load
        let prefs = preferred_targets(&caps);
        assert!(
            !prefs
                .iter()
                .any(|t| matches!(t, ArtifactTarget::QnnHtpV81 | ArtifactTarget::QnnHtpV73))
        );
        assert_eq!(prefs.first(), Some(&ArtifactTarget::CpuInt8));
    }

    #[test]
    fn preferred_targets_picks_the_detected_hexagon_generation() {
        let mut caps = Capabilities::unknown();
        caps.npu.present = true;
        caps.providers.qnn = true;
        caps.npu.htp_arch = Some(crate::capabilities::HtpArch::V73);
        let prefs = preferred_targets(&caps);
        assert_eq!(prefs.first(), Some(&ArtifactTarget::QnnHtpV73));
        assert!(!prefs.contains(&ArtifactTarget::QnnHtpV81));
    }
}
