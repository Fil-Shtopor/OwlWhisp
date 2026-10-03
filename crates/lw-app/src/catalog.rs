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
    "The Fast recommendation uses an estimate from the model's speed tier and your hardware. \
     RTF and Accuracy use Zenbook A16 measurements; run Benchmark to fill On your machine.";

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
    /// The reference run shown in the Models table is fixed to the Zenbook A16, independent of
    /// the current computer. Prefer its accelerator run when several A16 runs exist.
    pub fn a16_reference(&self) -> Option<&MeasuredPoint> {
        self.measurements
            .iter()
            .filter(|m| m.machine.starts_with("ASUS Zenbook A16"))
            .max_by_key(|m| m.hardware.preference_rank())
    }

    /// The error rate measured for one language on the reference machine.
    ///
    /// Here rather than in a front end because choosing a model for a language is a question about
    /// data. Answering it from the blended figure picked Whisper turbo for Chinese, which is the
    /// worst of the four entries that claim Chinese.
    pub fn measured_for_language(&self, language: &str) -> Option<&LanguageRate> {
        if language.is_empty() {
            return None;
        }
        self.a16_reference()
            .and_then(|p| p.per_language.get(language))
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

/// Quarter-decade bucket, about 1.8x wide. Two numbers inside one bucket are treated as tied.
///
/// Used wherever the inputs cannot support a finer ordering: an estimate accurate to a factor of
/// two, or two error rates measured on different clip sets.
fn coarse(value: f32) -> i32 {
    (value.max(1e-6).log10() * 4.0).round() as i32
}

/// Order the candidates for one role by the thing that role actually claims.
///
/// Every role states a criterion in its own blurb -- fewest errors, least delay, smallest
/// download, most languages -- and for a long time all four were answered by one global ordering
/// led by the editorial quality tier. That tier is a judgement about a whole model, and it got the
/// per-language question wrong: Whisper turbo is `best` and so led the `accurate` pick for
/// Chinese while measuring CER 0.380 against SenseVoice's 0.141.
///
/// What decides how hard to compare is whether the numbers are the same measurement:
///
/// - **A language is chosen and both candidates were measured on it.** Same clips, same unit, so
///   the comparison is valid as it stands and the better number wins outright.
/// - **Anything else.** A blended rate is not one measurement: GigaAM's 0.029 comes from three
///   Russian clips and Whisper turbo's 0.042 from twelve across four languages, and a
///   one-language specialist sits an easier exam. Those are compared only as coarsely as that
///   allows, and the tie goes to language coverage -- the question was "any language", so a model
///   that speaks one cannot be the answer to it. `Fast` is always in this case, because its input
///   is an estimate rather than a measurement.
///
/// Returns `None` when the criterion cannot rank an entry at all; such entries sort last.
fn role_key(entry: &EntryView, role: ModelRole, language: &str) -> Option<(i32, i32)> {
    match role {
        ModelRole::Accurate => {
            if let Some(m) = entry.measured_for_language(language) {
                // Same clips for every candidate that has this: no bucketing, no tiebreak.
                return Some(((m.rate * 1e6) as i32, 0));
            }
            let blended = entry.a16_reference()?.wer?;
            Some((coarse(blended), -(entry.languages.len() as i32)))
        }
        ModelRole::Fast => {
            let rtf = entry.estimated_rtf?;
            Some((coarse(rtf), -(entry.languages.len() as i32)))
        }
        // Exact and never a tie; an entry with no pinned file set has no size to rank on.
        ModelRole::Compact => entry.download_bytes.map(|b| (b.min(i32::MAX as u64) as i32, 0)),
        ModelRole::Universal => Some((-(entry.languages.len() as i32), 0)),
    }
}

/// The entry to suggest for one role, optionally restricted to a language.
///
/// `language` is a code or the empty string for "any". Returns `None` when nothing in the catalog
/// carries the role, or carries it and claims the language.
pub fn pick_for_role<'a>(
    entries: &'a [EntryView],
    role: ModelRole,
    language: &str,
) -> Option<&'a EntryView> {
    entries
        .iter()
        .filter(|e| e.roles.iter().any(|r| r.id == role.id()))
        .filter(|e| language.is_empty() || e.languages.iter().any(|l| l == language))
        // `min_by_key` keeps the first of equals, and `entries` arrives in the catalog's own
        // best-first order, so anything the criterion cannot separate keeps that order.
        .min_by_key(|e| role_key(e, role, language).unwrap_or((i32::MAX, i32::MAX)))
}

/// Every language any entry carries a measurement for: in practice, what the fixture set covers.
///
/// Needed to tell "we measured this and it is bad" apart from "nobody measured this", which look
/// identical on screen and mean opposite things.
pub fn measured_languages(entries: &[EntryView]) -> std::collections::BTreeSet<String> {
    entries
        .iter()
        .flat_map(|e| e.measurements.iter())
        .flat_map(|m| m.per_language.keys().cloned())
        .collect()
}

/// Models with no credited maker.
pub const OTHER_VENDOR: &str = "Other";

/// Display name for an engine id, e.g. `nemo_transducer` -> `NeMo`.
///
/// The engine id says which loader runs the files; the family is what the model is *called*, and
/// two different engine ids can be the same family. Unknown ids are title-cased rather than
/// hidden, so a catalog that outlives this binary still reads sensibly.
pub fn family_label(engine: &str) -> String {
    match engine {
        "parakeet_tdt" => "Parakeet".into(),
        "nemo_transducer" | "nemo_ctc" => "NeMo".into(),
        "whisper" => "Whisper".into(),
        "moonshine" => "Moonshine".into(),
        "sense_voice" => "SenseVoice".into(),
        "paraformer" => "Paraformer".into(),
        "zipformer" => "Zipformer".into(),
        "telespeech" => "TeleSpeech".into(),
        "fire_red_asr" => "FireRedASR".into(),
        "dolphin" => "Dolphin".into(),
        "canary" => "Canary".into(),
        "wenet_ctc" => "WeNet".into(),
        "sherpa" => "Sherpa".into(),
        other => {
            let words: Vec<String> = other
                .split('_')
                .filter(|w| !w.is_empty())
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                        None => String::new(),
                    }
                })
                .collect();
            if words.is_empty() {
                "—".into()
            } else {
                words.join(" ")
            }
        }
    }
}

/// One continuous list, split by who made each model.
///
/// Deliberately not a filter: every model stays on screen at once, and a heading plus a little
/// space is the whole of the grouping. Vendor tabs would hide most of the catalog behind a click,
/// which is the opposite of what a comparison table is for.
///
/// Order: the recommended model's maker leads, so the recommendation stays near the top; then the
/// makers offering the most models, alphabetically within a tie; then `Other`. It depends only on
/// the catalog, so it does not shuffle when a model is installed or selected. Within a group the
/// catalog's own order is kept untouched.
pub fn group_by_vendor<'a>(
    entries: &'a [EntryView],
    recommended: Option<&str>,
) -> Vec<(String, Vec<&'a EntryView>)> {
    let name_of = |e: &EntryView| -> String {
        match e.vendor.as_deref().map(str::trim) {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => OTHER_VENDOR.to_string(),
        }
    };

    let mut order: Vec<String> = Vec::new();
    let mut buckets: std::collections::HashMap<String, Vec<&EntryView>> = Default::default();
    for e in entries {
        let n = name_of(e);
        if !order.contains(&n) {
            order.push(n.clone());
        }
        buckets.entry(n).or_default().push(e);
    }

    let lead = recommended
        .and_then(|id| entries.iter().find(|e| e.id == id))
        .map(name_of)
        .filter(|v| v != OTHER_VENDOR);

    let mut groups: Vec<(String, Vec<&EntryView>)> = order
        .into_iter()
        .map(|v| {
            let list = buckets.remove(&v).unwrap_or_default();
            (v, list)
        })
        .collect();
    groups.sort_by(|a, b| {
        use std::cmp::Ordering;
        if a.0 == b.0 {
            return Ordering::Equal;
        }
        if let Some(lead) = lead.as_deref() {
            if a.0 == lead {
                return Ordering::Less;
            }
            if b.0 == lead {
                return Ordering::Greater;
            }
        }
        if a.0 == OTHER_VENDOR {
            return Ordering::Greater;
        }
        if b.0 == OTHER_VENDOR {
            return Ordering::Less;
        }
        b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0))
    });
    groups
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
    fn choosing_chinese_suggests_the_model_measured_best_on_chinese() {
        // The bug this logic exists for. Whisper turbo has the best blended figure in the catalog
        // and the worst Chinese of the four entries that claim it, so any ranking that answers a
        // per-language question with a whole-model number picks the worst option there is.
        let view = build(std::path::Path::new("nonexistent-models-root")).expect("builds");
        let pick = pick_for_role(&view.entries, ModelRole::Accurate, "zh")
            .expect("something accurate claims Chinese");
        assert_eq!(pick.id, "sense-voice-small", "picked {} instead", pick.id);
    }

    #[test]
    fn choosing_any_language_does_not_suggest_a_one_language_model() {
        // The regression the language-aware version caused: ranking blended rates directly
        // rewards narrowness, because a specialist is scored on an easier set of clips. Both
        // suggestions went to a Russian-only model.
        let view = build(std::path::Path::new("nonexistent-models-root")).expect("builds");
        for role in [ModelRole::Fast, ModelRole::Accurate] {
            let pick = pick_for_role(&view.entries, role, "").expect("something fills the role");
            assert!(
                pick.languages.len() > 1,
                "{:?} for any language suggested {}, which claims {} language(s)",
                role,
                pick.id,
                pick.languages.len()
            );
        }
    }

    #[test]
    fn a_real_speed_difference_still_beats_language_coverage() {
        // The coverage tiebreak must stay a tiebreak. Whisper turbo claims a hundred languages and
        // is a whole tier slower, so it must not take the Fast pick from Parakeet.
        let view = build(std::path::Path::new("nonexistent-models-root")).expect("builds");
        let fast = pick_for_role(&view.entries, ModelRole::Fast, "").expect("a fast pick");
        assert_ne!(fast.id, "whisper-turbo");
    }

    #[test]
    fn a_role_nothing_fills_for_a_language_has_no_pick() {
        let view = build(std::path::Path::new("nonexistent-models-root")).expect("builds");
        // Moonshine is the only compact entry and it is English-only.
        assert!(pick_for_role(&view.entries, ModelRole::Compact, "zh").is_none());
    }

    #[test]
    fn the_measured_languages_are_the_ones_with_fixtures() {
        let view = build(std::path::Path::new("nonexistent-models-root")).expect("builds");
        let measured = measured_languages(&view.entries);
        for code in ["en", "es", "ru", "uk", "zh"] {
            assert!(measured.contains(code), "{code} has fixtures but no measurement");
        }
        assert!(
            !measured.contains("yue"),
            "there are no Cantonese fixtures; claiming otherwise would let a pick look founded"
        );
    }

    #[test]
    fn a_language_rate_is_read_from_the_a16_reference() {
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

    #[test]
    fn parakeet_reference_uses_the_a16_npu_run_on_every_machine() {
        let view = build(std::path::Path::new("nonexistent-models-root")).expect("builds");
        let parakeet = view.entries.iter().find(|e| e.id == "parakeet-tdt-0.6b-v3").unwrap();
        let reference = parakeet.a16_reference().expect("A16 run");
        assert_eq!(reference.hardware, HardwareTarget::QnnNpu);
        assert!((reference.rtf - 0.0145).abs() < 1e-6);
        assert_eq!(reference.wer, Some(0.048));
        assert!((parakeet.measured_for_language("en").unwrap().rate - 0.092).abs() < 1e-6);
    }
}
