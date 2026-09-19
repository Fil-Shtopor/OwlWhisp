//! The model catalog as a front end shows it.
//!
//! This used to build a `serde_json::Value` inside a Tauri command, mirrored by hand in 775 lines
//! of TypeScript. The mirror drifted: the backend omits an absent `Option` rather than writing
//! null, the TypeScript declared the field non-optional, and the model list printed `NaN%` at a
//! user. Typed structs make that particular failure impossible, and they are what a native front
//! end needs anyway -- it cannot render a `Value` without re-deriving every field name.
//!
//! The field names are deliberately identical to the old JSON keys, so `serde_json::to_value` of
//! these types produces the payload the existing web UI already expects. That is what lets the two
//! front ends coexist while one replaces the other.

use std::path::{Path, PathBuf};

use lw_core::capabilities::Capabilities;
use lw_core::model::{
    Catalog, HardwareTarget, InstallState, LanguageRate, MeasuredPoint, ModelRole, QualityTier,
    SpeedTier, entry_paths, manifests_dir,
};
use serde::Serialize;

/// The sentence a front end must show beside any estimated number.
pub const ESTIMATE_DISCLAIMER: &str =
    "Estimated from the model's speed tier and your detected hardware - not a measurement. \
     Run the benchmark for a real number on this machine.";

/// One of the jobs an entry can be tagged with, as shown.
#[derive(Clone, Debug, Serialize)]
pub struct RoleView {
    pub id: &'static str,
    pub label: &'static str,
    pub blurb: &'static str,
}

impl From<ModelRole> for RoleView {
    fn from(r: ModelRole) -> Self {
        Self {
            id: r.id(),
            label: r.label(),
            blurb: r.blurb(),
        }
    }
}

/// Whether one accelerator can run one model, and why not when it cannot.
#[derive(Clone, Debug, Serialize)]
pub struct AcceleratorView {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: &'static str,
    pub supported: bool,
    /// Present exactly when `supported` is false. Deliberately does not name the accelerator --
    /// see `CatalogEntry::unsupported_on` -- so a front end can group accelerators by reason.
    pub reason: Option<String>,
}

/// One catalog entry with everything a front end needs to draw its row and its expansion.
#[derive(Clone, Debug, Serialize)]
pub struct EntryView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub engine: String,
    pub vendor: Option<String>,
    pub roles: Vec<RoleView>,
    pub licence: String,
    pub upstream_url: Option<String>,
    pub languages: Vec<String>,
    /// Names beside the codes rather than instead of them: the codes are what the manifests and
    /// `--languages` take, so the expanded row still shows them.
    pub language_names: Vec<String>,
    pub language_summary: String,
    pub quality: QualityTier,
    pub quality_label: &'static str,
    pub speed: SpeedTier,
    pub speed_label: &'static str,
    pub download_bytes: Option<u64>,
    pub disk_bytes: Option<u64>,
    pub hardware: Vec<&'static str>,
    /// Which accelerators this entry could run on, answered from the catalog rather than by
    /// trying. The same call decides what a Compare-all sweep skips, so the table and the
    /// benchmark can never disagree about it.
    pub accelerators: Vec<AcceleratorView>,
    pub runnable: bool,
    pub best_hardware: Option<&'static str>,
    /// An ESTIMATE. Never show it without the mark that says so.
    pub estimated_rtf: Option<f32>,
    pub measured_reference: Option<MeasuredPoint>,
    pub measurements: Vec<MeasuredPoint>,
    pub wer_estimates: std::collections::BTreeMap<String, f32>,
    pub reason: String,
    pub blockers: Vec<String>,
    pub install_state: InstallState,
    pub install_dir: Option<String>,
    pub manifest_error: Option<String>,
    pub notes: Option<String>,
}

impl EntryView {
    /// The error rate measured for one language, from the point this machine's path would use.
    ///
    /// Here rather than in a front end because choosing a model for a language is a question about
    /// data. Answering it from the blended figure picked Whisper turbo for Chinese, which is the
    /// worst of the four entries that claim Chinese.
    pub fn measured_for_language(&self, language: &str) -> Option<&LanguageRate> {
        if language.is_empty() {
            return None;
        }
        let want = self
            .best_hardware
            .map(|h| h.replace('_', "-").to_ascii_lowercase());
        let matches = |p: &&MeasuredPoint| {
            want.as_deref().is_some_and(|w| {
                p.hardware.label().replace('_', "-").to_ascii_lowercase() == w
            })
        };
        let preferred = self.measurements.iter().filter(matches);
        let rest = self.measurements.iter().filter(|p| !matches(p));
        preferred
            .chain(rest)
            .find_map(|p| p.per_language.get(language))
    }
}

/// The whole catalog as a front end shows it.
#[derive(Clone, Debug, Serialize)]
pub struct CatalogView {
    pub machine: String,
    pub models_root: String,
    pub manifests_dir: Option<String>,
    pub recommended: Option<String>,
    pub estimate_disclaimer: &'static str,
    /// The role vocabulary itself, so a picker offers exactly the roles this build knows about
    /// rather than a list hardcoded in a front end that could drift from the catalog.
    pub roles: Vec<RoleView>,
    pub entries: Vec<EntryView>,
}

/// Engine features this binary was built with, which decide what it can actually run.
pub fn engine_features() -> Vec<&'static str> {
    let mut f = vec!["parakeet"];
    // Ask the engine crate rather than testing a feature flag on this crate: it knows whether its
    // native library is linked, and this keeps every front end and the CLI from disagreeing.
    if lw_engine_sherpa::is_available() {
        f.push("sherpa");
    }
    f
}

/// Build the catalog view for a models directory.
pub fn build(root: &Path) -> Result<CatalogView, String> {
    let catalog = Catalog::builtin().map_err(|e| e.to_string())?;
    let caps: &Capabilities = crate::machine::probe_capabilities();
    let manifests = manifests_dir(None);
    let features = engine_features();
    let recs = catalog.recommend_with(caps, &features);

    // `recommend_with` sorts best-first, so the first runnable entry is the recommendation.
    let recommended = recs.iter().find(|r| r.runnable).map(|r| r.entry.id.clone());

    let entries = recs
        .iter()
        .map(|r| {
            let e = r.entry;
            let paths = entry_paths(e, manifests.as_deref(), root, caps);
            EntryView {
                id: e.id.clone(),
                name: e.name.clone(),
                description: e.description.clone(),
                engine: e.engine.as_str().to_string(),
                vendor: (!e.vendor.is_empty()).then(|| e.vendor.clone()),
                roles: e.roles.iter().copied().map(RoleView::from).collect(),
                licence: e.license.clone(),
                upstream_url: e.source_url.clone(),
                languages: e.languages.clone(),
                language_names: e.language_names(),
                language_summary: e.language_summary(),
                quality: e.quality,
                quality_label: e.quality.label(),
                speed: e.speed,
                speed_label: e.speed.label(),
                download_bytes: e.download_bytes,
                disk_bytes: e.disk_bytes,
                hardware: e.hardware.iter().map(|h| h.label()).collect(),
                accelerators: lw_core::capabilities::accel::ALL_ACCELERATORS
                    .iter()
                    .filter(|a| a.supported_on_this_platform())
                    .map(|a| {
                        let why = e.unsupported_on(*a);
                        AcceleratorView {
                            id: a.id(),
                            label: a.label(),
                            kind: a.kind().label(),
                            supported: why.is_none(),
                            reason: why,
                        }
                    })
                    .collect(),
                runnable: r.runnable,
                best_hardware: r.best_hardware.map(HardwareTarget::label),
                estimated_rtf: r.estimated_rtf,
                measured_reference: r.measured_reference.clone(),
                measurements: e.measurements.clone(),
                wer_estimates: e.wer_estimates.clone(),
                reason: r.reason.clone(),
                blockers: r.blockers.clone(),
                install_state: paths.state,
                install_dir: paths.dir.map(|d| d.display().to_string()),
                manifest_error: paths.manifest_error,
                notes: (!e.notes.is_empty()).then(|| e.notes.clone()),
            }
        })
        .collect();

    Ok(CatalogView {
        machine: caps.summary(),
        models_root: root.display().to_string(),
        manifests_dir: manifests.map(|d| d.display().to_string()),
        recommended,
        estimate_disclaimer: ESTIMATE_DISCLAIMER,
        roles: ModelRole::ALL.iter().copied().map(RoleView::from).collect(),
        entries,
    })
}

/// Where models live for a given settings file.
///
/// Deliberately not `default_models_root`: the app owns its data directory, and the two must not
/// disagree about what is installed.
pub fn models_root_for(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("models")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact keys `app/frontend/src/ipc.ts` reads. While the web front end still exists, a
    /// rename here breaks it at runtime and nowhere else -- there is no compiler spanning the two.
    /// When the web front end is gone this test goes with it.
    const ENTRY_KEYS: &[&str] = &[
        "id", "name", "description", "engine", "vendor", "roles", "licence", "upstream_url",
        "languages", "language_names", "language_summary", "quality", "quality_label", "speed",
        "speed_label", "download_bytes", "disk_bytes", "hardware", "accelerators", "runnable",
        "best_hardware", "estimated_rtf", "measured_reference", "measurements", "wer_estimates",
        "reason", "blockers", "install_state", "install_dir", "manifest_error", "notes",
    ];

    const TOP_KEYS: &[&str] = &[
        "machine", "models_root", "manifests_dir", "recommended", "estimate_disclaimer", "roles",
        "entries",
    ];

    #[test]
    fn the_serialized_shape_is_the_one_the_web_front_end_reads() {
        let view = build(std::path::Path::new("nonexistent-models-root"))
            .expect("the builtin catalog builds without a models directory");
        let v = serde_json::to_value(&view).expect("serializes");
        let obj = v.as_object().expect("an object");
        for key in TOP_KEYS {
            assert!(obj.contains_key(*key), "top level lost the key {key}");
        }
        assert_eq!(obj.len(), TOP_KEYS.len(), "top level gained a key: {:?}", obj.keys());

        let first = obj["entries"].as_array().expect("entries").first().cloned().expect("an entry");
        let e = first.as_object().expect("an object");
        for key in ENTRY_KEYS {
            assert!(e.contains_key(*key), "an entry lost the key {key}");
        }
        assert_eq!(e.len(), ENTRY_KEYS.len(), "an entry gained a key: {:?}", e.keys());
    }

    #[test]
    fn a_language_rate_is_read_from_the_path_this_machine_would_use() {
        let view = build(std::path::Path::new("nonexistent-models-root")).expect("builds");
        let sense = view
            .entries
            .iter()
            .find(|e| e.id == "sense-voice-small")
            .expect("sense-voice-small is in the catalog");
        let zh = sense
            .measured_for_language("zh")
            .expect("its Chinese was measured and stored");
        assert!((zh.rate - 0.141).abs() < 1e-3, "{}", zh.rate);
        assert!(sense.measured_for_language("ja").is_none(), "no Japanese fixtures exist");
        assert!(sense.measured_for_language("").is_none(), "no language chosen is not a language");
    }
}
