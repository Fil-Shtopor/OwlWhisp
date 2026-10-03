//! Building an engine for the current settings, and measuring one.
//!
//! Lifted out of the Tauri command layer unchanged. None of it ever needed a window: it loads the
//! settings, resolves the runtime and the model directory, builds an engine for the requested
//! backend, and runs the fixture set through it. The only thing the old home gave it was a place
//! to be called from, and now two front ends need to call it.

use std::path::PathBuf;

use lw_core::dictionary::Dictionary;
use lw_core::engine::{BackendPreference, EngineInitContext, SpeechEngine};
use lw_core::settings::Settings;
use lw_core::text::{
    CleanupProcessor, DictionaryProcessor, NormalizeOptions, TextPipeline, TextProcessor,
};
use lw_engine_parakeet::{BackendKind, ParakeetConfig, ParakeetEngine};

pub fn app_data_dir(settings_path: &std::path::Path) -> PathBuf {
    settings_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default()
}

pub fn runtime_dir() -> Option<PathBuf> {
    // `LW_RUNTIME_DIR`, then `runtime/<platform>/` and `runtime/` beside the executable.
    lw_ort::locate_runtime_dir()
}

pub struct Loaded {
    pub engine: Box<dyn SpeechEngine>,
    pub settings: Settings,
}

pub fn load_engine(settings_path: &std::path::Path) -> Result<Loaded, String> {
    let settings = Settings::load(settings_path).map_err(|e| e.to_string())?;
    let data = app_data_dir(settings_path);
    let model_dir = data.join("models").join(&settings.model_id);
    let cache_dir = data.join("cache");
    if !model_dir.exists() {
        return Err(format!(
            "model '{}' is not installed ({})",
            settings.model_id,
            model_dir.display()
        ));
    }
    let ctx = EngineInitContext {
        model_dir: model_dir.clone(),
        cache_dir: cache_dir.clone(),
        cpu_threads: 0,
    };

    // Pick the engine from what is actually in the model directory, so switching models in
    // Settings is enough to switch engines — no per-model wiring in the UI.
    let mut engine = build_engine_for(&model_dir, &settings, &ctx)?;
    engine.initialize(&ctx).map_err(|e| e.to_string())?;
    Ok(Loaded { engine, settings })
}

/// Choose an engine for the model directory's contents.
///
/// A sherpa-onnx style layout (Whisper / Moonshine / SenseVoice / NeMo transducer) goes to
/// [`lw_engine_sherpa`]; anything else is treated as the Parakeet TDT layout, which is the only one
/// that can use the Qualcomm NPU.
pub fn build_engine_for(
    model_dir: &std::path::Path,
    settings: &Settings,
    ctx: &EngineInitContext,
) -> Result<Box<dyn SpeechEngine>, String> {
    // Parakeet has a sherpa-compatible layout, but its built-in engine supports both CPU and
    // accelerators. Always use that engine for this model: the same installed download then
    // works with Automatic/CPU and a later explicit CUDA selection, even without sherpa.
    let built_in_parakeet = settings.model_id == "parakeet-tdt-0.6b-v3";
    if !built_in_parakeet
        && let Ok(files) = lw_engine_sherpa::detect_in_dir(model_dir, true, None)
    {
        if !matches!(settings.backend, BackendPreference::Automatic | BackendPreference::ForceCpu) {
            return Err(format!(
                "model '{}' uses the CPU-only sherpa engine; choose Parakeet TDT 0.6B v3 for CUDA or select CPU",
                settings.model_id
            ));
        }
        let kind = files.kind();
        #[cfg(feature = "sherpa")]
        {
            tracing::info!(
                "model '{}' detected as {kind}; using the sherpa engine",
                settings.model_id
            );
            let cfg = lw_engine_sherpa::SherpaConfig::new(model_dir);
            return Ok(Box::new(lw_engine_sherpa::SherpaEngine::new(cfg)));
        }
        #[cfg(not(feature = "sherpa"))]
        {
            return Err(format!(
                "model '{}' is a {kind} model, which needs a build with the `sherpa` engine feature",
                settings.model_id
            ));
        }
    }

    let rt_dir = runtime_dir().ok_or_else(|| "ONNX Runtime not found (set LW_RUNTIME_DIR)".to_string())?;
    #[cfg(windows)]
    if settings.backend == BackendPreference::DirectMl {
        return Ok(Box::new(crate::provider_worker::DirectMlEngine::new(
            crate::provider_worker::directml_runtime_dir(&rt_dir),
        )));
    }
    let runtime = lw_ort::OrtRuntime::init(&rt_dir).map_err(|e| e.to_string())?;
    let backend = BackendKind::from(settings.backend);
    // Hexagon generation comes from the detected NPU (V73 on X Elite / X Plus, V81 on X2 Elite),
    // never from a hard-coded assumption about one SoC.
    let config =
        ParakeetConfig::from_ctx(ctx, backend).with_capabilities(crate::machine::probe_capabilities());
    Ok(Box::new(ParakeetEngine::new(runtime, config)))
}

pub fn build_pipeline(settings: &Settings) -> TextPipeline {
    let mut pipeline = TextPipeline::new();
    // Deterministic cleanup (whitespace + NFC always; casing/punctuation are conservative defaults).
    pipeline.push(Box::new(CleanupProcessor::new(NormalizeOptions::default())) as Box<dyn TextProcessor>);
    if !settings.dictionary.rules.is_empty() {
        let dict: Dictionary = settings.dictionary.clone();
        pipeline.push(Box::new(DictionaryProcessor::new(dict)));
    }
    pipeline
}

/// What the dictation engine is running on right now.
///
/// `loaded` is false until the first dictation, because the engine loads lazily — and the honest
/// answer before that is "nothing yet", not a prediction from settings.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ActiveBackend {
    /// Whether an engine is loaded at all.
    pub loaded: bool,
    /// The execution provider that actually ran, read back from the engine.
    pub provider: Option<String>,
    /// Its acceleration class (CPU / GPU / NPU / ANE), for display.
    pub acceleration: Option<String>,
    /// The selected accelerator's stable id (`"qnn_npu"`, `"web_gpu"`, …). Compare against this,
    /// never against the display strings above.
    pub accelerator: Option<String>,
    /// That accelerator's class as a stable id (`"cpu"` / `"gpu"` / `"npu"`).
    pub accelerator_kind: Option<String>,
    /// Device description.
    pub device: Option<String>,
    /// Backend-selection notes, including the reason for any fallback.
    pub notes: Vec<String>,
    /// The model it loaded.
    pub model_id: String,
    /// Why loading failed, when it did.
    pub error: Option<String>,
}

/// How many fixture clips a benchmark uses.
///
/// The whole committed set is fifteen (three each in en/ru/es/uk/zh). Taking all of them costs a few
/// seconds per backend -- engine load dominates a run, not transcription -- and in exchange the
/// WER covers four languages rather than only English. `measure` scores only the languages the
/// model claims, so an English-only model is still judged on English alone.
pub const BENCH_CLIPS: usize = 15;

/// A benchmark report: every number in it was measured on this machine by this run.
#[derive(Clone, Debug, serde::Serialize)]
pub struct BenchReport {
    /// One-line description of the machine.
    pub machine: String,
    /// The backend that **actually** executed, read back from the engine rather than requested.
    pub backend: String,
    /// That backend's stable accelerator id, for comparing runs without matching display prose.
    pub accelerator: Option<String>,
    /// The model that was measured.
    pub model_id: String,
    /// Where its files were loaded from.
    pub model_dir: String,
    /// Where the audio came from, including whether WER was computable.
    pub clip_source: String,
    /// Time spent constructing and initializing the engine.
    pub engine_load_ms: f32,
    /// Per-clip results in run order.
    pub clips: Vec<lw_core::bench::ClipResult>,
    /// First run, which includes one-time warm-up.
    pub cold_rtf: f32,
    /// Mean of the runs after the first, or `None` when there was no second run: reporting a
    /// "warm" number derived from a single cold run would be a measurement of nothing.
    pub warm_rtf: Option<f32>,
    /// How many runs contributed to `warm_rtf`.
    pub warm_count: usize,
    /// Token-weighted error rate, or `None` when the clips carried no reference transcripts **or**
    /// they spanned both words and characters -- see `mixed_units`.
    pub wer: Option<f32>,
    /// What `wer` is measured in: `"word"` or `"character"`, when there is a figure.
    pub unit: Option<&'static str>,
    /// One real total per unit the run scored in, words first.
    ///
    /// Always populated; `wer` is just this list when it has exactly one entry. A run covering
    /// Russian and Chinese has two, and this is what a caller shows instead of nothing.
    pub by_unit: Vec<lw_core::bench::UnitScore>,
    /// True when the scored clips mixed word-scored and character-scored languages, so no single
    /// figure exists. Chinese is not measured in words and Russian is not measured in characters,
    /// and averaging the two would produce a number with no unit.
    pub mixed_units: bool,
    /// Total seconds of audio processed. Counts only the clips that were actually run.
    pub audio_secs: f32,
    /// Clips that were not run at all, because this model does not claim their language.
    ///
    /// They are absent from `clips` and from every figure above, which is the point: an
    /// English-only model still produces *something* for Chinese audio, and the time it spends
    /// doing so would otherwise be averaged into its RTF as though it were real work.
    pub skipped_clips: usize,
    /// The languages `skipped_clips` were in, sorted.
    pub skipped_languages: Vec<String>,
    /// The languages the error rate above actually covers, sorted.
    pub scored_languages: Vec<String>,
    /// What the engine decided while selecting a backend -- including why it fell back, if it did.
    pub notes: Vec<String>,
}

/// One accelerator that was offered but not measured, and why.
#[derive(Clone, Debug, serde::Serialize)]
pub struct BenchSkipped {
    /// Stable accelerator id.
    pub accelerator: String,
    /// Human label.
    pub label: String,
    /// Why it was skipped or how it failed.
    pub reason: String,
}

/// Every accelerator measured on the same clips, so the numbers can honestly be compared.
#[derive(Clone, Debug, serde::Serialize)]
pub struct BenchSuite {
    /// One-line description of the machine.
    pub machine: String,
    /// The model every run used.
    pub model_id: String,
    /// Where the audio came from. One clip set for all runs -- that is what makes this a comparison.
    pub clip_source: String,
    /// One report per accelerator that ran, in the order they were measured.
    pub runs: Vec<BenchReport>,
    /// Accelerators that could not be measured, with the reason.
    pub skipped: Vec<BenchSkipped>,
    /// The id of the fastest run by warm RTF, or `None` if nothing ran.
    pub fastest: Option<String>,
    /// The id of the most accurate run by word-weighted WER, when WER was computable.
    pub most_accurate: Option<String>,
}

/// Measure every usable accelerator on one clip set and return the comparison.
///
/// Emits `benchmark_progress` as each accelerator starts and finishes, because a full sweep takes
/// minutes -- a first NPU run alone can spend several preparing its context binary.
pub fn run_benchmark_suite(
    settings_path: &std::path::Path,
    model_id: Option<String>,
    mut on_progress: impl FnMut(serde_json::Value),
) -> Result<BenchSuite, String> {
    let settings = Settings::load(settings_path).map_err(|e| e.to_string())?;
    let model = model_id.unwrap_or(settings.model_id);
    let caps = crate::machine::probe_capabilities();

    let runtime_dir = runtime_dir().ok_or_else(|| "ONNX Runtime not found".to_string())?;
    let runtime = lw_ort::OrtRuntime::init(&runtime_dir).map_err(|e| e.to_string())?;
    let mut usable = runtime.usable_accelerators();
    #[cfg(windows)]
    if crate::provider_worker::probe_directml(&runtime_dir).is_ok_and(|count| count > 0) {
        let insert_at = usable
            .iter()
            .position(|accel| *accel == lw_core::capabilities::Accelerator::WebGpu)
            .unwrap_or(usable.len());
        usable.insert(insert_at, lw_core::capabilities::Accelerator::DirectMl);
    }
    if usable.is_empty() {
        return Err("no usable accelerator on this machine".into());
    }

    // Load once. Every backend must see identical audio or the comparison means nothing.
    let clips = lw_core::bench::quick_clips(None, BENCH_CLIPS).map_err(|e| e.to_string())?;
    let clip_source = clips.1.describe();
    let total = usable.len();

    // What the catalog says about this model, so an accelerator it cannot possibly use is skipped
    // rather than attempted. Without the entry we try everything, which is the old behaviour and
    // the right fallback: guessing "unsupported" from a missing catalog row would hide a model
    // that works.
    let entry = lw_core::model::Catalog::builtin()
        .ok()
        .and_then(|c| c.get(&model).cloned());

    let mut runs = Vec::new();
    let mut skipped = Vec::new();
    for (i, accel) in usable.iter().enumerate() {
        if let Some(reason) = entry.as_ref().and_then(|e| e.unsupported_on(*accel)) {
            on_progress(serde_json::json!({
                "index": i, "total": total,
                "accelerator": accel.id(), "label": accel.label(),
                "stage": "skipped", "reason": reason.clone(),
            }));
            skipped.push(BenchSkipped {
                accelerator: accel.id().to_string(),
                label: accel.label().to_string(),
                reason,
            });
            continue;
        }
        on_progress(serde_json::json!({
            "index": i, "total": total,
            "accelerator": accel.id(), "label": accel.label(),
            "stage": "running",
        }));
        let pref = lw_core::engine::BackendPreference::for_accelerator(*accel);
        match run_benchmark_job(settings_path, Some(model.clone()), Some(pref), Some(&clips)) {
            Ok(report) => {
                on_progress(serde_json::json!({
                    "index": i, "total": total,
                    "accelerator": accel.id(), "label": accel.label(),
                    "stage": "done",
                    "warm_rtf": report.warm_rtf, "wer": report.wer,
                }));
                runs.push(report);
            }
            Err(reason) => {
                on_progress(serde_json::json!({
                    "index": i, "total": total,
                    "accelerator": accel.id(), "label": accel.label(),
                    "stage": "failed", "reason": reason.clone(),
                }));
                skipped.push(BenchSkipped {
                    accelerator: accel.id().to_string(),
                    label: accel.label().to_string(),
                    reason,
                });
            }
        }
    }

    // "Fastest" uses the warm figure: the cold run carries one-time setup that says nothing about
    // steady-state dictation. A run with no warm figure is not eligible rather than being ranked
    // on a number that means something else.
    let fastest = runs
        .iter()
        .filter_map(|r| r.warm_rtf.map(|w| (r, w)))
        .filter(|(_, w)| w.is_finite() && *w > 0.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .and_then(|(r, _)| r.accelerator.clone());
    let most_accurate = runs
        .iter()
        .filter_map(|r| r.wer.map(|w| (r, w)))
        .filter(|(_, w)| w.is_finite())
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .and_then(|(r, _)| r.accelerator.clone());

    Ok(BenchSuite {
        machine: caps.summary(),
        model_id: model,
        clip_source,
        runs,
        skipped,
        fastest,
        most_accurate,
    })
}

/// Build the requested engine, measure it on real (or, failing that, synthetic) clips, and report.
///
/// Overrides are applied to a *copy* of the settings, so measuring a model the user has not
/// selected never changes what dictation uses.
pub fn run_benchmark_job(
    settings_path: &std::path::Path,
    model_id: Option<String>,
    backend: Option<BackendPreference>,
    clips: Option<&(Vec<lw_core::bench::Clip>, lw_core::bench::ClipSource)>,
) -> Result<BenchReport, String> {
    let mut settings = Settings::load(settings_path).map_err(|e| e.to_string())?;
    if let Some(id) = model_id {
        settings.model_id = id;
    }
    if let Some(b) = backend {
        settings.backend = b;
    }

    let data = app_data_dir(settings_path);
    let model_dir = data.join("models").join(&settings.model_id);
    if !model_dir.exists() {
        return Err(format!(
            "model '{}' is not installed ({})",
            settings.model_id,
            model_dir.display()
        ));
    }
    let ctx = EngineInitContext {
        model_dir: model_dir.clone(),
        cache_dir: data.join("cache"),
        cpu_threads: 0,
    };

    let t0 = std::time::Instant::now();
    let mut engine = build_engine_for(&model_dir, &settings, &ctx)?;
    engine.initialize(&ctx).map_err(|e| e.to_string())?;
    let engine_load_ms = t0.elapsed().as_secs_f32() * 1000.0;

    // The whole fixture set is offered; `measure` then runs only the clips whose language this
    // model claims, so an English-only model is timed on English and a Russian-only model on
    // Russian. Transcription is not what makes a sweep slow -- loading an engine is, and a first
    // NPU run additionally prepares a context binary -- so offering the full set costs a few
    // seconds per backend and buys a WER over five languages instead of three English clips.
    // Comparing backends is only meaningful on identical audio, so a suite loads the clips once
    // and hands the same set to every run.
    let owned;
    let (clips, source) = match clips {
        Some(c) => (&c.0, c.1.clone()),
        None => {
            owned = lw_core::bench::quick_clips(None, BENCH_CLIPS).map_err(|e| e.to_string())?;
            (&owned.0, owned.1.clone())
        }
    };
    let clip_source = source.describe();
    // What the catalog says this model speaks, for the engines whose files carry no language
    // metadata of their own (GigaAM v3, Parakeet TDT-CTC 110M). Without it those two get timed on
    // every fixture language, which is exactly the averaging-in of work they were never built for
    // that skipping exists to stop. Ignored when the engine does declare its own languages.
    let assume_languages = lw_core::model::Catalog::builtin()
        .ok()
        .and_then(|c| {
            c.entries
                .iter()
                .find(|e| e.id == settings.model_id)
                .map(|e| e.languages.clone())
        })
        .unwrap_or_default();
    let opts = lw_core::bench::MeasureOptions {
        assume_languages,
        ..Default::default()
    };
    let m = lw_core::bench::measure_with(engine.as_mut(), clips, source, opts, |_| {})
        .map_err(|e| e.to_string())?;

    let report = BenchReport {
        machine: crate::machine::probe_capabilities().summary(),
        // Read back what ran, not what was asked for: a requested NPU run that fell back to CPU
        // must say CPU.
        backend: format!("{} on {}", engine.provider(), engine.acceleration()),
        accelerator: engine.accelerator().map(|a| a.id().to_string()),
        model_id: settings.model_id.clone(),
        model_dir: model_dir.display().to_string(),
        clip_source,
        engine_load_ms,
        clips: m.results,
        cold_rtf: m.cold_rtf,
        warm_rtf: (m.warm_count > 0).then_some(m.warm_rtf),
        warm_count: m.warm_count,
        wer: m.wer,
        unit: m.unit.map(lw_core::bench::ErrorUnit::label),
        by_unit: m.by_unit.clone(),
        mixed_units: m.mixed_units,
        audio_secs: m.audio_secs,
        skipped_clips: m.skipped_clips,
        skipped_languages: m.skipped_languages.clone(),
        scored_languages: m.scored_languages.clone(),
        notes: engine.notes().to_vec(),
    };
    engine.shutdown();
    Ok(report)
}

/// Map an RMS amplitude to a meter position in `[0, 1]`.
///
/// A linear RMS bar is useless: ordinary speech sits around 0.02-0.2, so it would barely leave
/// the left edge while clipping would look like a third of the scale. Hearing is closer to
/// logarithmic, so this uses the usual voice-meter range, -60 dBFS at the left to 0 dBFS at the
/// right. It is a display mapping of a real measurement, not a substitute for one.
pub fn meter_level(rms: f32) -> f32 {
    const FLOOR_DB: f32 = -60.0;
    if !rms.is_finite() || rms <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * rms.clamp(1e-6, 1.0).log10();
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}
