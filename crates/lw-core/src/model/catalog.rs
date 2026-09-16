//! The **model catalog**: a pre-download description of the speech models LocalWisper knows about,
//! plus a hardware-aware recommender.
//!
//! The catalog answers three questions *before* anything is downloaded:
//!
//! 1. **What exists?** — id, engine, languages, download/disk size, licence-relevant notes.
//! 2. **Will it run here?** — which of CPU / Qualcomm-NPU / CoreML the entry has artifacts for,
//!    intersected with what the machine actually offers (see [`crate::capabilities`]).
//! 3. **Roughly how fast/accurate?** — a *transparent, documented estimate* plus, separately, any
//!    numbers that were really measured.
//!
//! # Estimates vs measurements — the central distinction
//!
//! This module is deliberately pedantic about the difference:
//!
//! - [`Recommendation::estimated_rtf`] is an **ESTIMATE**. It is produced by
//!   [`estimate_rtf`] from the entry's coarse [`SpeedTier`] scaled by a hardware factor. It has
//!   never been run on your machine, or possibly on any machine. It exists to rank models and to
//!   set expectations ("this will be comfortably real-time" vs "this will be slow"), not to be
//!   quoted.
//! - [`CatalogEntry::measurements`] are **MEASUREMENTS**. Each [`MeasuredPoint`] carries the exact
//!   machine it was taken on and a citation for where the number came from. A measurement taken on
//!   somebody else's machine is still a measurement — but it is a measurement *of that machine*,
//!   which is why the machine string is mandatory.
//! - [`CatalogEntry::wer_estimates`] are published or previously-measured accuracy figures keyed by
//!   language code. They are labelled "estimates" because WER depends entirely on the test set;
//!   the only WER you can trust for your audio is one you measured on your audio.
//!
//! Nothing in this module ever promotes an estimate to a measurement, and the CLI renders the two
//! in separate, separately-labelled columns.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::capabilities::Capabilities;
use crate::{Error, Result};

/// The catalog that ships with the binary (`models/catalog.json` at the workspace root).
pub const BUILTIN_CATALOG_JSON: &str = include_str!("../../../../models/catalog.json");

/// The catalog schema version this build understands.
pub const CATALOG_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------------------------
// Small enums
// ---------------------------------------------------------------------------------------------

/// Which engine implementation is required to run a catalog entry.
///
/// Serialized as a plain snake_case string (`"parakeet_tdt"`, `"whisper"`, `"moonshine"`); any
/// other string round-trips through [`EngineKind::Other`] so a newer catalog never fails to parse
/// on an older binary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum EngineKind {
    /// NVIDIA Parakeet TDT (the built-in engine, `lw-engine-parakeet`).
    ParakeetTdt,
    /// OpenAI Whisper encoder/decoder.
    Whisper,
    /// Moonshine (Useful Sensors / Moonshine AI).
    Moonshine,
    /// An engine this build does not know about.
    Other(String),
}

impl EngineKind {
    /// The canonical snake_case string form.
    pub fn as_str(&self) -> &str {
        match self {
            EngineKind::ParakeetTdt => "parakeet_tdt",
            EngineKind::Whisper => "whisper",
            EngineKind::Moonshine => "moonshine",
            EngineKind::Other(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for EngineKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<String> for EngineKind {
    fn from(s: String) -> Self {
        match s.as_str() {
            "parakeet_tdt" => EngineKind::ParakeetTdt,
            "whisper" => EngineKind::Whisper,
            "moonshine" => EngineKind::Moonshine,
            _ => EngineKind::Other(s),
        }
    }
}

impl From<EngineKind> for String {
    fn from(e: EngineKind) -> Self {
        match e {
            EngineKind::Other(s) => s,
            other => other.as_str().to_string(),
        }
    }
}

/// A compute target a catalog entry has artifacts for.
///
/// This is the *catalog-level* view (what the model was built for). The finer-grained per-file
/// keying used when downloading lives in [`crate::model::ArtifactTarget`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareTarget {
    /// ONNX Runtime CPU execution provider. Always available.
    Cpu,
    /// Qualcomm Hexagon NPU via the ORT QNN execution provider.
    QnnNpu,
    /// Apple CoreML execution provider (ANE / GPU).
    CoreMl,
}

impl HardwareTarget {
    /// Preference rank when several targets are available; higher is preferred.
    pub fn preference_rank(self) -> u8 {
        match self {
            HardwareTarget::QnnNpu => 2,
            HardwareTarget::CoreMl => 1,
            HardwareTarget::Cpu => 0,
        }
    }

    /// Short label for tables.
    pub fn label(self) -> &'static str {
        match self {
            HardwareTarget::Cpu => "cpu",
            HardwareTarget::QnnNpu => "qnn-npu",
            HardwareTarget::CoreMl => "coreml",
        }
    }
}

impl std::fmt::Display for HardwareTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Coarse transcription-quality tier, ascending.
///
/// This is an editorial ranking of the model family, not a measurement. Compare
/// [`CatalogEntry::wer_estimates`] and [`CatalogEntry::measurements`] for numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    /// Usable for short commands; expect errors on hard audio.
    Basic,
    /// Fine for everyday dictation in clean conditions.
    Good,
    /// Solid general-purpose accuracy.
    Better,
    /// Best available in this catalog.
    Best,
}

impl QualityTier {
    /// Short label for tables.
    pub fn label(self) -> &'static str {
        match self {
            QualityTier::Basic => "basic",
            QualityTier::Good => "good",
            QualityTier::Better => "better",
            QualityTier::Best => "best",
        }
    }
}

impl std::fmt::Display for QualityTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Coarse speed tier, ascending (later variants are faster).
///
/// The tier is the *input* to the RTF estimate in [`estimate_rtf`]; it is not itself a
/// measurement. Each tier maps to a base real-time factor on a documented reference machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedTier {
    /// Noticeably slower than real time on a laptop CPU.
    Slow,
    /// Faster than real time, but you will feel the wait on long utterances.
    Moderate,
    /// Comfortably faster than real time.
    Fast,
    /// Effectively instant for dictation-length audio.
    VeryFast,
}

impl SpeedTier {
    /// Short label for tables.
    pub fn label(self) -> &'static str {
        match self {
            SpeedTier::Slow => "slow",
            SpeedTier::Moderate => "moderate",
            SpeedTier::Fast => "fast",
            SpeedTier::VeryFast => "very-fast",
        }
    }
}

impl std::fmt::Display for SpeedTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

// ---------------------------------------------------------------------------------------------
// Measurements
// ---------------------------------------------------------------------------------------------

/// A number that was **actually measured**, with the machine and citation that make it meaningful.
///
/// Never construct one of these from a guess. If you do not have a real run to point at, leave the
/// entry's `measurements` list empty — an honest blank beats a plausible fabrication.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeasuredPoint {
    /// The compute target the run used.
    pub hardware: HardwareTarget,
    /// Measured real-time factor (wall-clock seconds per second of audio; lower is faster).
    pub rtf: f32,
    /// Measured word error rate as a fraction in `(0, 1]`, if the run scored one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wer: Option<f32>,
    /// The machine the run happened on. Mandatory: a measurement without a machine is folklore.
    pub machine: String,
    /// Where the number came from (document section, tool, date).
    pub source: String,
}

// ---------------------------------------------------------------------------------------------
// Entries
// ---------------------------------------------------------------------------------------------

/// One model in the catalog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Stable catalog id, e.g. `"parakeet-tdt-0.6b-v3"`. Unique within a catalog.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The engine implementation required to run it.
    pub engine: EngineKind,
    /// One-line description shown in `lw models list`.
    pub description: String,
    /// True if the model handles more than one language.
    #[serde(default)]
    pub multilingual: bool,
    /// Supported language codes.
    pub languages: Vec<String>,
    /// Bytes transferred to install, if known. `None` = not verified; never guessed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_bytes: Option<u64>,
    /// Bytes occupied on disk after install, if known. `None` = not verified; never guessed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_bytes: Option<u64>,
    /// Editorial quality tier.
    pub quality: QualityTier,
    /// Editorial speed tier — the input to the RTF **estimate**.
    pub speed: SpeedTier,
    /// Published or previously-measured WER by language code, as a fraction in `(0, 1]`.
    ///
    /// These are **estimates for your audio**: WER is a property of a test set, not of a model.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub wer_estimates: BTreeMap<String, f32>,
    /// Numbers that were really measured, each tagged with the machine it was measured on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurements: Vec<MeasuredPoint>,
    /// Compute targets this entry ships artifacts for.
    pub hardware: Vec<HardwareTarget>,
    /// Cargo/engine feature that must be built in to run this entry (e.g. `"sherpa"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_engine_feature: Option<String>,
    /// File name of the hash-pinned manifest under `models/manifests/` that describes the actual
    /// files, e.g. `"parakeet-tdt-0.6b-v3.json"`.
    ///
    /// `None` means **no pinned file set yet** — the entry is informational and `lw models install`
    /// must refuse it rather than fetch something unverified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// Upstream URL the files come from, for humans. Informational only: downloads always go
    /// through the hash-pinned manifest, never through this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Licence string for the model weights.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub license: String,
    /// Honest free-text note: what is verified, what is estimated, what is missing.
    pub notes: String,
}

impl CatalogEntry {
    /// The measurement for a given hardware target, if the catalog has one.
    pub fn measurement_for(&self, hw: HardwareTarget) -> Option<&MeasuredPoint> {
        self.measurements.iter().find(|m| m.hardware == hw)
    }

    /// True if this entry ships artifacts for `hw`.
    pub fn supports(&self, hw: HardwareTarget) -> bool {
        self.hardware.contains(&hw)
    }

    /// A short language summary, e.g. `"en"` or `"25 languages (en, es, fr, …)"`.
    pub fn language_summary(&self) -> String {
        match self.languages.len() {
            0 => "—".to_string(),
            1 => self.languages[0].clone(),
            n if n <= 3 => self.languages.join(", "),
            n => format!("{n}: {}, …", self.languages[..3].join(", ")),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Estimation heuristic
// ---------------------------------------------------------------------------------------------

/// The reference machine the [`SpeedTier`] base RTFs are defined against: an 8-performance-core
/// ARM64 laptop CPU running the ONNX Runtime CPU execution provider.
pub const REFERENCE_CPU_CORES: f32 = 8.0;

/// Ratio applied to the CPU estimate when the encoder can run on a Qualcomm NPU.
///
/// Anchored on the single measured pair we have: Parakeet TDT 0.6B v3 end-to-end on the Snapdragon
/// X2 Elite, CPU RTF 0.032 vs NPU RTF 0.0145 (`docs/benchmarks.md` §5.1) → 0.45. It has **not**
/// been validated for any other model, and end-to-end speedup is capped by the CPU-side mel and
/// TDT decode stages, so it is far smaller than the encoder-only speedup.
pub const NPU_RTF_RATIO: f32 = 0.45;

/// Ratio applied to the CPU estimate when CoreML is available.
///
/// **Unvalidated.** We have no Apple hardware in the benchmark set (`docs/benchmarks.md` §6); this
/// is a deliberately conservative placeholder, not a claim.
pub const COREML_RTF_RATIO: f32 = 0.60;

/// The most credit a many-core machine gets over the 8-core reference (1 / 0.75 ≈ 1.33×).
///
/// Small on purpose: end-to-end RTF is dominated by the single-threaded mel front end and TDT
/// decode, and on the X2 Elite 18 cores measured no faster than the 8-core reference figure.
pub const MIN_CORE_SCALE: f32 = 0.75;

/// The worst penalty a tiny or unidentified machine takes (4× the reference RTF).
pub const MAX_CORE_SCALE: f32 = 4.0;

/// Base real-time factor for a speed tier on [`REFERENCE_CPU_CORES`].
///
/// The `VeryFast` anchor (0.035) is set from the measured Parakeet CPU RTF of 0.032 on a 12-core
/// Snapdragon X2 Elite; the other tiers are ordinal steps away from it, not measurements.
pub fn base_rtf(tier: SpeedTier) -> f32 {
    match tier {
        SpeedTier::Slow => 0.60,
        SpeedTier::Moderate => 0.25,
        SpeedTier::Fast => 0.08,
        SpeedTier::VeryFast => 0.035,
    }
}

/// **ESTIMATE ONLY.** Predict a real-time factor for `tier` on `hardware`, given `cpu_cores`.
///
/// The heuristic is deliberately simple and fully documented so a reader can judge it:
///
/// 1. Start from [`base_rtf`] for the tier (defined on an 8-core reference CPU).
/// 2. Scale inversely with usable CPU cores, clamped to 2..=16 cores and then to a
///    [`MIN_CORE_SCALE`]..[`MAX_CORE_SCALE`] factor. The two clamps are deliberately asymmetric: a
///    small or unidentified machine gets a pessimistic 4× penalty, while a big machine is credited
///    with at most a 1.33× speedup, because the mel front end and the per-frame TDT decode are
///    single-threaded and only the encoder scales with cores. (On the one machine we measured, 18
///    cores were no faster end to end than the 8-core reference — `docs/benchmarks.md` §5.1.)
/// 3. If the chosen target is an accelerator, multiply by [`NPU_RTF_RATIO`] or
///    [`COREML_RTF_RATIO`].
///
/// The result is a rough order-of-magnitude expectation. It is not, and must never be presented
/// as, a measurement. Run `lw bench --quick` for a real number on the real machine.
pub fn estimate_rtf(tier: SpeedTier, hardware: HardwareTarget, cpu_cores: usize) -> f32 {
    let cores = cpu_cores.clamp(2, 16) as f32;
    let core_scale = (REFERENCE_CPU_CORES / cores).clamp(MIN_CORE_SCALE, MAX_CORE_SCALE);
    let cpu_estimate = base_rtf(tier) * core_scale;
    match hardware {
        HardwareTarget::Cpu => cpu_estimate,
        HardwareTarget::QnnNpu => cpu_estimate * NPU_RTF_RATIO,
        HardwareTarget::CoreMl => cpu_estimate * COREML_RTF_RATIO,
    }
}

/// The compute targets a machine can actually offer, best first.
///
/// An accelerator counts only when the *execution provider* for it is available, not merely when
/// the OS reports the hardware: `capabilities::detect()` can see a Hexagon driver package while
/// the QNN EP still fails to load. Callers that want the NPU considered must fill
/// [`crate::capabilities::AvailableProviders::qnn`] from a real ONNX Runtime probe first.
pub fn available_targets(caps: &Capabilities) -> Vec<HardwareTarget> {
    let mut out = Vec::new();
    if caps.npu.present && caps.providers.qnn {
        out.push(HardwareTarget::QnnNpu);
    }
    if caps.providers.coreml {
        out.push(HardwareTarget::CoreMl);
    }
    // The CPU EP is the universal fallback and is never removed.
    out.push(HardwareTarget::Cpu);
    out
}

// ---------------------------------------------------------------------------------------------
// Recommendations
// ---------------------------------------------------------------------------------------------

/// The result of matching one [`CatalogEntry`] against one machine.
///
/// `estimated_rtf` is an **estimate** (see [`estimate_rtf`]); `measured_reference` is a real
/// measurement, but on the machine named inside it — which may not be yours.
#[derive(Clone, Debug, Serialize)]
pub struct Recommendation<'a> {
    /// The catalog entry this recommendation is about.
    pub entry: &'a CatalogEntry,
    /// True if this build, on this machine, could actually run the model today.
    pub runnable: bool,
    /// The best compute target available here for this entry, if any.
    pub best_hardware: Option<HardwareTarget>,
    /// **ESTIMATED** real-time factor on `best_hardware`. Never a measurement.
    pub estimated_rtf: Option<f32>,
    /// A real measurement from the catalog for `best_hardware`, if one exists. Read
    /// [`MeasuredPoint::machine`] before believing it applies to you.
    pub measured_reference: Option<MeasuredPoint>,
    /// Short human explanation of the verdict.
    pub reason: String,
    /// Everything standing between this machine and running the model.
    pub blockers: Vec<String>,
}

impl Recommendation<'_> {
    /// Sort key: runnable first, then better quality, then lower estimated RTF, then id.
    fn sort_key(&self) -> (u8, std::cmp::Reverse<QualityTier>, u32, &str) {
        let rtf_bits = self
            .estimated_rtf
            .map(|r| (r.max(0.0) * 1e6) as u32)
            .unwrap_or(u32::MAX);
        (
            u8::from(!self.runnable),
            std::cmp::Reverse(self.entry.quality),
            rtf_bits,
            self.entry.id.as_str(),
        )
    }
}

// ---------------------------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------------------------

/// A parsed, validated catalog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// Schema version; this build understands [`CATALOG_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The models.
    pub entries: Vec<CatalogEntry>,
}

impl Catalog {
    /// Parse a catalog from JSON **and validate it**.
    pub fn from_json(json: &str) -> Result<Self> {
        let cat: Catalog =
            serde_json::from_str(json).map_err(|e| Error::Model(format!("catalog parse: {e}")))?;
        cat.validate()?;
        Ok(cat)
    }

    /// The catalog compiled into this binary (`models/catalog.json`).
    pub fn builtin() -> Result<Self> {
        Self::from_json(BUILTIN_CATALOG_JSON)
    }

    /// Read and validate a catalog from a file.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path.display().to_string(), e))?;
        Self::from_json(&text)
    }

    /// Look up an entry by id.
    pub fn get(&self, id: &str) -> Option<&CatalogEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Iterate the entries in catalog order.
    pub fn iter(&self) -> std::slice::Iter<'_, CatalogEntry> {
        self.entries.iter()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if the catalog has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Structural validation: known schema, unique non-empty ids, populated required fields, sane
    /// sizes, in-range error rates, and safe manifest file names.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_SCHEMA_VERSION {
            return Err(Error::Model(format!(
                "unsupported catalog schema_version {} (expected {CATALOG_SCHEMA_VERSION})",
                self.schema_version
            )));
        }
        if self.entries.is_empty() {
            return Err(Error::Model("catalog has no entries".into()));
        }
        // 1 TiB: anything larger is a units mistake, not a speech model.
        const MAX_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
        let mut seen: std::collections::BTreeSet<&str> = Default::default();
        for e in &self.entries {
            let bad = |m: String| Err(Error::Model(m));
            if e.id.trim().is_empty() {
                return bad("catalog entry with empty id".into());
            }
            if !e
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
            {
                return bad(format!("entry id has unsafe characters: {}", e.id));
            }
            if !seen.insert(e.id.as_str()) {
                return bad(format!("duplicate catalog id: {}", e.id));
            }
            if e.name.trim().is_empty() {
                return bad(format!("{}: empty name", e.id));
            }
            if e.description.trim().is_empty() {
                return bad(format!("{}: empty description", e.id));
            }
            if e.notes.trim().is_empty() {
                return bad(format!(
                    "{}: empty notes (every entry must state what is verified)",
                    e.id
                ));
            }
            if e.languages.is_empty() {
                return bad(format!("{}: no languages listed", e.id));
            }
            if e.multilingual != (e.languages.len() > 1) {
                return bad(format!(
                    "{}: multilingual={} contradicts {} language(s)",
                    e.id,
                    e.multilingual,
                    e.languages.len()
                ));
            }
            if e.hardware.is_empty() {
                return bad(format!("{}: no hardware targets listed", e.id));
            }
            for (field, v) in [("download_bytes", e.download_bytes), ("disk_bytes", e.disk_bytes)] {
                if let Some(v) = v
                    && (v == 0 || v > MAX_BYTES)
                {
                    return bad(format!("{}: implausible {field}: {v}", e.id));
                }
            }
            if let (Some(dl), Some(disk)) = (e.download_bytes, e.disk_bytes)
                && disk < dl
            {
                return bad(format!(
                    "{}: disk_bytes {disk} < download_bytes {dl} (an install cannot shrink)",
                    e.id
                ));
            }
            for (lang, wer) in &e.wer_estimates {
                if !(0.0..=1.0).contains(wer) || !wer.is_finite() {
                    return bad(format!("{}: wer_estimates[{lang}] out of range: {wer}", e.id));
                }
            }
            for m in &e.measurements {
                if !(m.rtf.is_finite() && m.rtf > 0.0) {
                    return bad(format!("{}: measurement rtf out of range: {}", e.id, m.rtf));
                }
                if let Some(w) = m.wer
                    && (!(0.0..=1.0).contains(&w) || !w.is_finite())
                {
                    return bad(format!("{}: measurement wer out of range: {w}", e.id));
                }
                if m.machine.trim().is_empty() || m.source.trim().is_empty() {
                    return bad(format!(
                        "{}: a measurement must name the machine it ran on and cite a source",
                        e.id
                    ));
                }
                if !e.supports(m.hardware) {
                    return bad(format!(
                        "{}: measurement for {} but the entry lists no such artifact",
                        e.id, m.hardware
                    ));
                }
            }
            if let Some(man) = &e.manifest {
                if man.trim().is_empty() || !man.ends_with(".json") {
                    return bad(format!("{}: manifest must be a *.json file name", e.id));
                }
                if man.contains('/') || man.contains('\\') || man.contains("..") {
                    return bad(format!("{}: manifest must be a bare file name: {man}", e.id));
                }
            }
            if let Some(u) = &e.source_url
                && !u.starts_with("https://")
            {
                return bad(format!("{}: non-https source_url: {u}", e.id));
            }
        }
        Ok(())
    }

    /// Match every entry against this machine, best first.
    ///
    /// Assumes **no optional engine features are compiled in** — entries with a
    /// [`CatalogEntry::requires_engine_feature`] are reported as not runnable. Use
    /// [`Catalog::recommend_with`] to declare the features this build actually has.
    ///
    /// Each [`Recommendation::estimated_rtf`] is an estimate; see [`estimate_rtf`].
    pub fn recommend(&self, caps: &Capabilities) -> Vec<Recommendation<'_>> {
        self.recommend_with(caps, &[])
    }

    /// [`Catalog::recommend`], but with the set of optional engine features this build has.
    pub fn recommend_with(&self, caps: &Capabilities, engine_features: &[&str]) -> Vec<Recommendation<'_>> {
        let machine = available_targets(caps);
        let machine_has_npu = machine.contains(&HardwareTarget::QnnNpu);
        let unknown_machine = caps.cpu_brand.trim().is_empty() || caps.cpu_cores <= 1;

        let mut out: Vec<Recommendation<'_>> = self
            .entries
            .iter()
            .map(|entry| {
                let mut blockers = Vec::new();

                if let Some(feat) = &entry.requires_engine_feature
                    && !engine_features.contains(&feat.as_str())
                {
                    blockers.push(format!(
                        "needs the `{feat}` engine feature, which this build does not include"
                    ));
                }

                // Best target both the entry and the machine offer.
                let best_hardware = machine
                    .iter()
                    .copied()
                    .filter(|hw| entry.supports(*hw))
                    .max_by_key(|hw| hw.preference_rank());
                if best_hardware.is_none() {
                    blockers.push(format!(
                        "no artifact for this machine (model targets: {})",
                        entry
                            .hardware
                            .iter()
                            .map(|h| h.label())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }

                let runnable = blockers.is_empty();
                let estimated_rtf = best_hardware.map(|hw| estimate_rtf(entry.speed, hw, caps.cpu_cores));
                let measured_reference = best_hardware.and_then(|hw| entry.measurement_for(hw)).cloned();

                let reason = if let Some(first) = blockers.first() {
                    first.clone()
                } else {
                    match best_hardware {
                        Some(HardwareTarget::QnnNpu) => {
                            "runs the encoder on the Hexagon NPU (ORT QNN EP)".to_string()
                        }
                        Some(HardwareTarget::CoreMl) => {
                            "can use the CoreML path — unvalidated, never benchmarked on Apple hardware"
                                .to_string()
                        }
                        Some(HardwareTarget::Cpu) if machine_has_npu => {
                            "CPU only — this model ships no NPU artifact".to_string()
                        }
                        Some(HardwareTarget::Cpu) if caps.npu.present => {
                            "CPU only — an NPU is present but its execution provider is not available"
                                .to_string()
                        }
                        Some(HardwareTarget::Cpu) if unknown_machine => {
                            "CPU only — machine not identified, so this estimate is deliberately pessimistic"
                                .to_string()
                        }
                        Some(HardwareTarget::Cpu) => "CPU only — no NPU detected".to_string(),
                        None => "not runnable here".to_string(),
                    }
                };

                Recommendation {
                    entry,
                    runnable,
                    best_hardware,
                    estimated_rtf,
                    measured_reference,
                    reason,
                    blockers,
                }
            })
            .collect();

        out.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{Capabilities, HtpArch};

    fn npu_machine() -> Capabilities {
        let mut c = Capabilities::unknown();
        c.cpu_brand = "Snapdragon X2 Elite X2E94100".into();
        c.cpu_cores = 12;
        c.is_qualcomm = true;
        c.npu.present = true;
        c.npu.htp_arch = Some(HtpArch::V81);
        c.providers.qnn = true;
        c
    }

    fn cpu_machine() -> Capabilities {
        let mut c = Capabilities::unknown();
        c.cpu_brand = "Intel Core i7-1260P".into();
        c.cpu_cores = 12;
        c
    }

    fn entry(id: &str) -> CatalogEntry {
        CatalogEntry {
            id: id.into(),
            name: "Test".into(),
            engine: EngineKind::ParakeetTdt,
            description: "test entry".into(),
            multilingual: false,
            languages: vec!["en".into()],
            download_bytes: Some(1000),
            disk_bytes: Some(1000),
            quality: QualityTier::Good,
            speed: SpeedTier::Fast,
            wer_estimates: BTreeMap::new(),
            measurements: Vec::new(),
            hardware: vec![HardwareTarget::Cpu],
            requires_engine_feature: None,
            manifest: Some(format!("{id}.json")),
            source_url: None,
            license: "CC-BY-4.0".into(),
            notes: "synthetic test entry; nothing here is measured".into(),
        }
    }

    fn catalog(entries: Vec<CatalogEntry>) -> Catalog {
        Catalog {
            schema_version: 1,
            entries,
        }
    }

    // ----- validation -----

    #[test]
    fn valid_catalog_passes() {
        catalog(vec![entry("a")]).validate().unwrap();
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let mut c = catalog(vec![entry("a")]);
        c.schema_version = 2;
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_empty_catalog() {
        assert!(catalog(vec![]).validate().is_err());
    }

    #[test]
    fn rejects_duplicate_ids() {
        let err = catalog(vec![entry("a"), entry("a")]).validate().unwrap_err();
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    #[test]
    fn rejects_unsafe_id() {
        let mut e = entry("a");
        e.id = "../evil".into();
        assert!(catalog(vec![e]).validate().is_err());
    }

    #[test]
    fn rejects_empty_notes() {
        let mut e = entry("a");
        e.notes = "  ".into();
        let err = catalog(vec![e]).validate().unwrap_err();
        assert!(err.to_string().contains("notes"), "{err}");
    }

    #[test]
    fn rejects_multilingual_inconsistency() {
        let mut e = entry("a");
        e.multilingual = true; // but only one language
        assert!(catalog(vec![e]).validate().is_err());
    }

    #[test]
    fn rejects_zero_and_absurd_sizes() {
        let mut e = entry("a");
        e.download_bytes = Some(0);
        assert!(catalog(vec![e.clone()]).validate().is_err());
        e.download_bytes = Some(u64::MAX);
        e.disk_bytes = None;
        assert!(catalog(vec![e]).validate().is_err());
    }

    #[test]
    fn rejects_disk_smaller_than_download() {
        let mut e = entry("a");
        e.download_bytes = Some(1000);
        e.disk_bytes = Some(999);
        assert!(catalog(vec![e]).validate().is_err());
    }

    #[test]
    fn rejects_out_of_range_wer() {
        let mut e = entry("a");
        e.wer_estimates.insert("en".into(), 42.0);
        assert!(catalog(vec![e]).validate().is_err());
    }

    #[test]
    fn rejects_measurement_without_machine() {
        let mut e = entry("a");
        e.measurements.push(MeasuredPoint {
            hardware: HardwareTarget::Cpu,
            rtf: 0.03,
            wer: None,
            machine: String::new(),
            source: "somewhere".into(),
        });
        let err = catalog(vec![e]).validate().unwrap_err();
        assert!(err.to_string().contains("machine"), "{err}");
    }

    #[test]
    fn rejects_measurement_for_unsupported_target() {
        let mut e = entry("a"); // cpu only
        e.measurements.push(MeasuredPoint {
            hardware: HardwareTarget::QnnNpu,
            rtf: 0.01,
            wer: None,
            machine: "X2".into(),
            source: "docs".into(),
        });
        assert!(catalog(vec![e]).validate().is_err());
    }

    #[test]
    fn rejects_manifest_with_path_separators() {
        let mut e = entry("a");
        e.manifest = Some("../../etc/passwd.json".into());
        assert!(catalog(vec![e]).validate().is_err());
    }

    #[test]
    fn rejects_non_https_source_url() {
        let mut e = entry("a");
        e.source_url = Some("http://example.com/m.tar".into());
        assert!(catalog(vec![e]).validate().is_err());
    }

    #[test]
    fn from_json_validates() {
        let bad = r#"{"schema_version":1,"entries":[]}"#;
        assert!(Catalog::from_json(bad).is_err());
    }

    // ----- the shipped catalog -----

    #[test]
    fn builtin_catalog_parses_and_validates() {
        let c = Catalog::builtin().unwrap();
        assert!(c.len() >= 4, "expected several entries, got {}", c.len());
        assert!(c.get("parakeet-tdt-0.6b-v3").is_some());
    }

    #[test]
    fn builtin_parakeet_carries_real_measurements() {
        let c = Catalog::builtin().unwrap();
        let e = c.get("parakeet-tdt-0.6b-v3").unwrap();
        let cpu = e.measurement_for(HardwareTarget::Cpu).unwrap();
        let npu = e.measurement_for(HardwareTarget::QnnNpu).unwrap();
        // The numbers from docs/benchmarks.md §5.1.
        assert!((cpu.rtf - 0.032).abs() < 1e-6);
        assert!((npu.rtf - 0.0145).abs() < 1e-6);
        assert!(npu.machine.contains("X2"));
        assert!(!cpu.source.is_empty());
    }

    #[test]
    fn builtin_manifest_references_resolve_to_a_valid_pinned_file() {
        // An entry that names a manifest promises `lw models install` something it can fetch and
        // verify. Check the promise is real: the file exists, parses, validates, and describes the
        // same model. (This replaced an older test asserting that feature-gated entries were
        // *never* pinned — true only while the sherpa file sets had no hashes of their own.)
        let manifests = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../models/manifests")
            .canonicalize()
            .expect("models/manifests");
        let c = Catalog::builtin().unwrap();
        let mut checked = 0;
        for e in c.iter() {
            let Some(name) = &e.manifest else { continue };
            let path = manifests.join(name);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("{}: {} -> {err}", e.id, path.display()));
            let m: crate::model::ModelManifest = serde_json::from_str(&text)
                .unwrap_or_else(|err| panic!("{}: {} -> {err}", e.id, path.display()));
            m.validate()
                .unwrap_or_else(|err| panic!("{}: {} -> {err}", e.id, path.display()));
            assert_eq!(m.id, e.id, "{} is pinned by a manifest for {}", e.id, m.id);
            checked += 1;
        }
        assert!(checked >= 2, "expected several pinned entries, got {checked}");
    }

    #[test]
    fn builtin_pinned_sizes_match_the_manifest_they_name() {
        // `download_bytes` is documented as a verified figure, so it must be the real sum of the
        // pinned file sizes — common files plus the CPU artifact set the installer would pick.
        let manifests = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../models/manifests")
            .canonicalize()
            .expect("models/manifests");
        let c = Catalog::builtin().unwrap();
        for e in c.iter() {
            let (Some(name), Some(want)) = (&e.manifest, e.download_bytes) else {
                continue;
            };
            let text = std::fs::read_to_string(manifests.join(name)).unwrap();
            let m: crate::model::ModelManifest = serde_json::from_str(&text).unwrap();
            let (_, files) = m
                .select_files(&[
                    crate::model::ArtifactTarget::CpuInt8,
                    crate::model::ArtifactTarget::Any,
                ])
                .unwrap_or_else(|| panic!("{}: manifest has no CPU artifact set", e.id));
            let got: u64 = files.iter().map(|f| f.bytes).sum();
            assert_eq!(got, want, "{}: download_bytes disagrees with {name}", e.id);
        }
    }

    // ----- estimation -----

    #[test]
    fn estimate_is_faster_on_npu_than_cpu() {
        let cpu = estimate_rtf(SpeedTier::VeryFast, HardwareTarget::Cpu, 12);
        let npu = estimate_rtf(SpeedTier::VeryFast, HardwareTarget::QnnNpu, 12);
        assert!(npu < cpu);
        assert!((npu / cpu - NPU_RTF_RATIO).abs() < 1e-5);
    }

    #[test]
    fn estimate_orders_by_speed_tier() {
        let f = |t| estimate_rtf(t, HardwareTarget::Cpu, 8);
        assert!(f(SpeedTier::VeryFast) < f(SpeedTier::Fast));
        assert!(f(SpeedTier::Fast) < f(SpeedTier::Moderate));
        assert!(f(SpeedTier::Moderate) < f(SpeedTier::Slow));
    }

    #[test]
    fn estimate_core_scaling_is_clamped() {
        // 2 cores and 1 core both clamp to the same pessimistic 4x factor.
        assert_eq!(
            estimate_rtf(SpeedTier::Fast, HardwareTarget::Cpu, 1),
            estimate_rtf(SpeedTier::Fast, HardwareTarget::Cpu, 2)
        );
        // 16 and 128 cores clamp to the same, deliberately modest, MIN_CORE_SCALE factor.
        assert_eq!(
            estimate_rtf(SpeedTier::Fast, HardwareTarget::Cpu, 16),
            estimate_rtf(SpeedTier::Fast, HardwareTarget::Cpu, 128)
        );
        // The reference machine itself gets the unscaled base RTF.
        assert!((estimate_rtf(SpeedTier::Fast, HardwareTarget::Cpu, 8) - 0.08).abs() < 1e-6);
        // Asymmetry: at most 1.33x faster, but up to 4x slower.
        let fastest = estimate_rtf(SpeedTier::Fast, HardwareTarget::Cpu, 64);
        let slowest = estimate_rtf(SpeedTier::Fast, HardwareTarget::Cpu, 1);
        assert!((fastest - 0.08 * MIN_CORE_SCALE).abs() < 1e-6, "{fastest}");
        assert!((slowest - 0.08 * MAX_CORE_SCALE).abs() < 1e-6, "{slowest}");
    }

    #[test]
    fn estimate_of_parakeet_cpu_is_within_2x_of_the_measured_value() {
        // Sanity-check the heuristic against the one end-to-end pair we actually measured
        // (docs/benchmarks.md §5.1: CPU 0.032, NPU 0.0145 on a 12-core X2 Elite). We only assert
        // an order-of-magnitude match — that is all an estimate is worth.
        let est_cpu = estimate_rtf(SpeedTier::VeryFast, HardwareTarget::Cpu, 12);
        let est_npu = estimate_rtf(SpeedTier::VeryFast, HardwareTarget::QnnNpu, 12);
        assert!((0.5..2.0).contains(&(est_cpu / 0.032)), "cpu estimate {est_cpu}");
        assert!((0.5..2.0).contains(&(est_npu / 0.0145)), "npu estimate {est_npu}");
    }

    // ----- recommendation -----

    #[test]
    fn npu_machine_prefers_the_npu_target() {
        let mut e = entry("parakeet");
        e.hardware = vec![HardwareTarget::Cpu, HardwareTarget::QnnNpu];
        let c = catalog(vec![e]);
        let rec = &c.recommend(&npu_machine())[0];
        assert!(rec.runnable);
        assert_eq!(rec.best_hardware, Some(HardwareTarget::QnnNpu));
        assert!(rec.reason.contains("Hexagon"), "{}", rec.reason);
    }

    #[test]
    fn cpu_only_machine_falls_back_to_cpu() {
        let mut e = entry("parakeet");
        e.hardware = vec![HardwareTarget::Cpu, HardwareTarget::QnnNpu];
        let c = catalog(vec![e]);
        let rec = &c.recommend(&cpu_machine())[0];
        assert!(rec.runnable);
        assert_eq!(rec.best_hardware, Some(HardwareTarget::Cpu));
        assert!(rec.reason.contains("no NPU detected"), "{}", rec.reason);
    }

    #[test]
    fn npu_machine_says_so_when_the_model_has_no_npu_artifact() {
        let c = catalog(vec![entry("cpu-only")]); // hardware = [Cpu]
        let rec = &c.recommend(&npu_machine())[0];
        assert_eq!(rec.best_hardware, Some(HardwareTarget::Cpu));
        assert!(rec.reason.contains("ships no NPU artifact"), "{}", rec.reason);
    }

    #[test]
    fn npu_hardware_without_its_provider_is_not_offered_as_a_target() {
        // The driver package is there, but the QNN EP never loaded: the honest answer is CPU, and
        // the reason must say why rather than claiming no NPU exists.
        let mut caps = npu_machine();
        caps.providers.qnn = false;
        let mut e = entry("parakeet");
        e.hardware = vec![HardwareTarget::Cpu, HardwareTarget::QnnNpu];
        let c = catalog(vec![e]);
        let rec = &c.recommend(&caps)[0];
        assert_eq!(rec.best_hardware, Some(HardwareTarget::Cpu));
        assert!(
            rec.reason.contains("execution provider is not available"),
            "{}",
            rec.reason
        );
    }

    #[test]
    fn unknown_machine_gets_a_pessimistic_cpu_verdict() {
        let c = catalog(vec![entry("a")]);
        let rec = &c.recommend(&Capabilities::unknown())[0];
        assert!(rec.runnable);
        assert_eq!(rec.best_hardware, Some(HardwareTarget::Cpu));
        assert!(rec.reason.contains("pessimistic"), "{}", rec.reason);
        // Unknown machines clamp to 2 cores -> the 4x pessimistic factor.
        assert!(rec.estimated_rtf.unwrap() > base_rtf(SpeedTier::Fast));
    }

    #[test]
    fn feature_gated_entry_is_not_runnable_by_default() {
        let mut e = entry("whisper-base");
        e.requires_engine_feature = Some("sherpa".into());
        let c = catalog(vec![e]);
        let rec = &c.recommend(&npu_machine())[0];
        assert!(!rec.runnable);
        assert_eq!(rec.blockers.len(), 1);
        assert!(rec.blockers[0].contains("sherpa"), "{}", rec.blockers[0]);
        // It still gets an estimate + target so the user can see what they would get.
        assert_eq!(rec.best_hardware, Some(HardwareTarget::Cpu));
        assert!(rec.estimated_rtf.is_some());
    }

    #[test]
    fn feature_gated_entry_becomes_runnable_when_the_feature_is_present() {
        let mut e = entry("whisper-base");
        e.requires_engine_feature = Some("sherpa".into());
        let c = catalog(vec![e]);
        let recs = c.recommend_with(&npu_machine(), &["sherpa"]);
        assert!(recs[0].runnable);
        assert!(recs[0].blockers.is_empty());
    }

    #[test]
    fn entry_with_no_matching_target_is_not_runnable() {
        let mut e = entry("apple-only");
        e.hardware = vec![HardwareTarget::CoreMl];
        let c = catalog(vec![e]);
        let rec = &c.recommend(&cpu_machine())[0];
        assert!(!rec.runnable);
        assert!(rec.best_hardware.is_none());
        assert!(rec.estimated_rtf.is_none());
        assert!(rec.blockers[0].contains("no artifact"), "{}", rec.blockers[0]);
    }

    #[test]
    fn ordering_is_runnable_then_quality_then_estimate() {
        let mut blocked = entry("z-blocked");
        blocked.quality = QualityTier::Best;
        blocked.requires_engine_feature = Some("sherpa".into());

        let mut good_fast = entry("b-good-fast");
        good_fast.quality = QualityTier::Good;
        good_fast.speed = SpeedTier::VeryFast;

        let mut best_slow = entry("a-best-slow");
        best_slow.quality = QualityTier::Best;
        best_slow.speed = SpeedTier::Slow;

        let c = catalog(vec![good_fast, blocked, best_slow]);
        let ids: Vec<&str> = c
            .recommend(&cpu_machine())
            .iter()
            .map(|r| r.entry.id.as_str())
            .collect();
        // Runnable first; among runnable, higher quality wins even though it is slower.
        assert_eq!(ids, vec!["a-best-slow", "b-good-fast", "z-blocked"]);
    }

    #[test]
    fn ordering_breaks_quality_ties_by_estimate_then_id() {
        let mut a = entry("aaa");
        a.speed = SpeedTier::Fast;
        let mut b = entry("bbb");
        b.speed = SpeedTier::VeryFast;
        let mut c_ = entry("ccc");
        c_.speed = SpeedTier::Fast;
        let c = catalog(vec![c_, a, b]);
        let ids: Vec<&str> = c
            .recommend(&cpu_machine())
            .iter()
            .map(|r| r.entry.id.as_str())
            .collect();
        assert_eq!(ids, vec!["bbb", "aaa", "ccc"]);
    }

    #[test]
    fn measured_reference_is_attached_only_for_the_chosen_target() {
        let mut e = entry("parakeet");
        e.hardware = vec![HardwareTarget::Cpu, HardwareTarget::QnnNpu];
        e.measurements = vec![MeasuredPoint {
            hardware: HardwareTarget::QnnNpu,
            rtf: 0.0145,
            wer: Some(0.048),
            machine: "Snapdragon X2 Elite".into(),
            source: "docs/benchmarks.md".into(),
        }];
        let c = catalog(vec![e]);
        assert!(c.recommend(&npu_machine())[0].measured_reference.is_some());
        // On a CPU-only machine the NPU measurement must NOT be shown as if it applied.
        assert!(c.recommend(&cpu_machine())[0].measured_reference.is_none());
    }

    // ----- misc -----

    #[test]
    fn engine_kind_roundtrips_unknown_values() {
        let json = "\"brand_new_engine\"";
        let k: EngineKind = serde_json::from_str(json).unwrap();
        assert_eq!(k, EngineKind::Other("brand_new_engine".into()));
        assert_eq!(serde_json::to_string(&k).unwrap(), json);
        assert_eq!(
            serde_json::to_string(&EngineKind::ParakeetTdt).unwrap(),
            "\"parakeet_tdt\""
        );
    }

    #[test]
    fn hardware_target_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&HardwareTarget::QnnNpu).unwrap(),
            "\"qnn_npu\""
        );
        assert_eq!(
            serde_json::to_string(&HardwareTarget::CoreMl).unwrap(),
            "\"core_ml\""
        );
    }

    #[test]
    fn tiers_are_ordinal() {
        assert!(QualityTier::Basic < QualityTier::Best);
        assert!(SpeedTier::Slow < SpeedTier::VeryFast);
    }

    #[test]
    fn language_summary_shapes() {
        let mut e = entry("a");
        assert_eq!(e.language_summary(), "en");
        e.languages = vec!["en".into(), "es".into(), "fr".into(), "de".into()];
        assert_eq!(e.language_summary(), "4: en, es, fr, …");
    }

    #[test]
    fn available_targets_always_include_cpu() {
        assert_eq!(
            available_targets(&Capabilities::unknown()),
            vec![HardwareTarget::Cpu]
        );
        assert_eq!(
            available_targets(&npu_machine()),
            vec![HardwareTarget::QnnNpu, HardwareTarget::Cpu]
        );
    }

    #[test]
    fn catalog_json_roundtrip() {
        let c = catalog(vec![entry("a")]);
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(Catalog::from_json(&s).unwrap(), c);
    }
}
