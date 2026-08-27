//! `lw models …` — browse, inspect, install and compare the model catalog, plus the shared
//! measurement code behind `lw bench --quick`.
//!
//! # Estimates vs measurements
//!
//! Every number this module prints falls into exactly one of two buckets and is labelled as such:
//!
//! - **EST RTF** — an estimate from [`lw_core::model::estimate_rtf`]: the catalog entry's coarse
//!   speed tier scaled by the detected hardware. It has never been run anywhere. Estimates are
//!   always prefixed with `~` and always sit under a column header containing "EST".
//! - **MEAS** — a real timing produced on this machine during this command, or a real timing from
//!   the catalog that carries the name of the machine it was taken on.
//!
//! The two are never merged into one column and an estimate is never described as a measurement.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use clap::Subcommand;
use lw_core::bench::{ClipResult, ClipSource, Measurement, measure, quick_clips};
use lw_core::capabilities::Capabilities;
use lw_core::engine::SpeechEngine;
use lw_ort::OrtRuntime;

use lw_core::model::{
    ArtifactTarget, CancellationToken, Catalog, EngineKind, EntryPaths, HardwareTarget, InstallState,
    ModelDownloader, ModelManifest, ModelRegistry, Recommendation, default_models_root, entry_paths,
    manifests_dir, preferred_targets, staging_dir_for,
};

use crate::BackendArg;

/// The one-line warning attached to every estimated number this CLI prints.
const ESTIMATE_DISCLAIMER: &str =
    "EST RTF is an ESTIMATE (speed tier scaled for the detected hardware), not a measurement.";

/// Where to go for a real number instead.
const MEASURE_HINT: &str =
    "Measure your own machine with `lw bench --quick --model-dir <dir>` or `lw models compare`.";

// ---------------------------------------------------------------------------------------------
// CLI surface
// ---------------------------------------------------------------------------------------------

/// `lw models <subcommand>`.
#[derive(Subcommand)]
pub enum ModelsCmd {
    /// List the catalog with sizes, tiers, install state and a hardware recommendation.
    List {
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Models root to check for installed models (default: the per-user models directory).
        #[arg(long)]
        dest: Option<PathBuf>,
        /// Catalog file to read instead of the one built into this binary.
        #[arg(long)]
        catalog: Option<PathBuf>,
        /// Directory holding the pinned `*.json` model manifests (default: auto-detect).
        #[arg(long)]
        manifests: Option<PathBuf>,
    },
    /// Show everything the catalog knows about one model.
    Info {
        /// Catalog id, e.g. `parakeet-tdt-0.6b-v3`.
        id: String,
        /// Emit JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Models root to check for installed models.
        #[arg(long)]
        dest: Option<PathBuf>,
        /// Catalog file to read instead of the one built into this binary.
        #[arg(long)]
        catalog: Option<PathBuf>,
        /// Directory holding the pinned `*.json` model manifests (default: auto-detect).
        #[arg(long)]
        manifests: Option<PathBuf>,
    },
    /// Download and verify a model from its pinned manifest (resumable, SHA-256 checked).
    Install {
        /// Catalog id to install.
        id: String,
        /// Models root to install into.
        #[arg(long)]
        dest: PathBuf,
        /// Force a specific artifact target instead of the best one for this machine.
        #[arg(long)]
        target: Option<String>,
        /// Catalog file to read instead of the one built into this binary.
        #[arg(long)]
        catalog: Option<PathBuf>,
        /// Directory holding the pinned `*.json` model manifests (default: auto-detect).
        #[arg(long)]
        manifests: Option<PathBuf>,
    },
    /// One table: estimated RTF for this hardware next to RTF/WER measured here, now.
    Compare {
        /// Models root to look for installed models in.
        #[arg(long)]
        dest: Option<PathBuf>,
        /// Fixtures directory (WAVs + `fixtures.json`) to measure with; enables WER.
        #[arg(long)]
        fixtures: Option<PathBuf>,
        /// Backend to measure on.
        #[arg(long, value_enum, default_value_t = BackendArg::Auto)]
        backend: BackendArg,
        /// Do not run any inference; show only catalog data and estimates.
        #[arg(long)]
        no_run: bool,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Catalog file to read instead of the one built into this binary.
        #[arg(long)]
        catalog: Option<PathBuf>,
        /// Directory holding the pinned `*.json` model manifests (default: auto-detect).
        #[arg(long)]
        manifests: Option<PathBuf>,
    },
}

/// Dispatch a `lw models` subcommand.
pub fn run(cmd: ModelsCmd, runtime_dir: &Option<PathBuf>) -> anyhow::Result<()> {
    match cmd {
        ModelsCmd::List {
            json,
            dest,
            catalog,
            manifests,
        } => list(runtime_dir, json, dest, catalog, manifests),
        ModelsCmd::Info {
            id,
            json,
            dest,
            catalog,
            manifests,
        } => info(runtime_dir, &id, json, dest, catalog, manifests),
        ModelsCmd::Install {
            id,
            dest,
            target,
            catalog,
            manifests,
        } => install(runtime_dir, &id, &dest, target.as_deref(), catalog, manifests),
        ModelsCmd::Compare {
            dest,
            fixtures,
            backend,
            no_run,
            json,
            catalog,
            manifests,
        } => compare(CompareArgs {
            runtime_dir: runtime_dir.clone(),
            dest,
            fixtures,
            backend,
            no_run,
            json,
            catalog,
            manifests,
        }),
    }
}

// ---------------------------------------------------------------------------------------------
// Paths and install state
// ---------------------------------------------------------------------------------------------

/// How hard to probe for accelerator execution providers.
///
/// Registering the QNN EP is a *global, one-way* change to the ONNX Runtime environment: once
/// registered, ORT auto-applies it to sessions that were meant to stay on the CPU, and a second
/// `OrtRuntime` handle cannot register it again. So the strong probe is only safe in commands that
/// never build a session afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe {
    /// Register the QNN EP and enumerate devices. Strongest answer, but mutates global ORT state.
    Enumerate,
    /// Only check that the QNN EP library is present next to the runtime. Leaves ORT untouched so
    /// an engine built later still decides when — and whether — to register it.
    LibraryPresent,
}

/// Detect the machine, then fill in `providers` from what the ONNX Runtime offers.
///
/// `lw_platform::caps::detect()` deliberately reports only what the OS can tell it — the Hexagon
/// driver package is on disk, the CPU is a Snapdragon — and leaves `providers.*` false, because
/// whether the QNN execution provider is really usable can only be answered by loading ORT. The
/// catalog recommender refuses to count an accelerator it has not seen a provider for, so we probe
/// here (best effort) and say plainly when we could not.
///
/// Returns the runtime handle as well: callers that go on to build an engine **must** reuse it
/// rather than initializing a second one.
fn detect_caps(
    runtime_dir: &Option<PathBuf>,
    probe: Probe,
) -> (Capabilities, Option<String>, Option<Arc<OrtRuntime>>) {
    let mut caps = lw_platform::caps::detect();
    caps.providers.cpu = true;
    match crate::init_runtime(runtime_dir) {
        Ok(rt) => {
            caps.providers.qnn = match probe {
                Probe::Enumerate => rt.has_qnn_npu(),
                Probe::LibraryPresent => caps.npu.present && rt.qnn_available(),
            };
            let note = (caps.npu.present && !caps.providers.qnn).then(|| match probe {
                Probe::Enumerate => "an NPU driver package is present but the QNN execution \
                                     provider did not enumerate a device — estimates fall back to CPU"
                    .to_string(),
                Probe::LibraryPresent => "an NPU driver package is present but the QNN \
                                          execution-provider library is missing from the runtime \
                                          directory — estimates fall back to CPU"
                    .to_string(),
            });
            (caps, note, Some(rt))
        }
        Err(e) => (
            caps,
            Some(format!(
                "ONNX Runtime not loaded ({e}); accelerator availability is unverified, so every \
                 estimate below assumes CPU"
            )),
            None,
        ),
    }
}

/// The per-user models root (`%LOCALAPPDATA%/LocalWisper/models`, or the XDG equivalent).
/// Load the catalog: an explicit file if given, otherwise the one compiled into this binary.
fn load_catalog(explicit: Option<PathBuf>) -> anyhow::Result<Catalog> {
    match explicit {
        Some(p) => Catalog::load(&p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display())),
        None => Catalog::builtin().map_err(|e| anyhow::anyhow!(e.to_string())),
    }
}

// ---------------------------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------------------------

/// Human byte size, or `—` when the catalog honestly does not know.
fn bytes_or_dash(b: Option<u64>) -> String {
    match b {
        None => "—".to_string(),
        Some(v) if v >= 1 << 30 => format!("{:.2} GiB", v as f64 / (1u64 << 30) as f64),
        Some(v) if v >= 1 << 20 => format!("{:.1} MiB", v as f64 / (1u64 << 20) as f64),
        Some(v) => format!("{v} B"),
    }
}

/// Truncate to `n` display columns with an ellipsis.
fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let keep: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{keep}…")
    }
}

// ---------------------------------------------------------------------------------------------
// `lw models list`
// ---------------------------------------------------------------------------------------------

fn list(
    runtime_dir: &Option<PathBuf>,
    json: bool,
    dest: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
    manifests_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let catalog = load_catalog(catalog_path)?;
    // `list` builds no sessions, so the strong probe is safe here.
    let (caps, caps_note, _rt) = detect_caps(runtime_dir, Probe::Enumerate);
    let root = dest.unwrap_or_else(default_models_root);
    let mdir = manifests_dir(manifests_path);
    let recs = catalog.recommend(&caps);

    let paths: Vec<EntryPaths> = recs
        .iter()
        .map(|r| entry_paths(r.entry, mdir.as_deref(), &root, &caps))
        .collect();

    // The top-ranked runnable entry is the one we mark as recommended.
    let recommended_id = recs.iter().find(|r| r.runnable).map(|r| r.entry.id.clone());

    if json {
        let models: Vec<serde_json::Value> = recs
            .iter()
            .zip(&paths)
            .map(|(r, p)| rec_json(r, p, recommended_id.as_deref()))
            .collect();
        let out = serde_json::json!({
            "schema_version": catalog.schema_version,
            "machine": {
                "cpu_brand": caps.cpu_brand,
                "cpu_cores": caps.cpu_cores,
                "npu_present": caps.npu.present,
                "npu_htp_arch": caps.npu.htp_arch.map(|a| a.num()),
                "qnn_provider": caps.providers.qnn,
                "coreml_provider": caps.providers.coreml,
            },
            "models_root": root.display().to_string(),
            "capability_note": caps_note,
            "estimate_disclaimer": ESTIMATE_DISCLAIMER,
            "models": models,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Model catalog (schema v{})", catalog.schema_version);
    println!("machine: {}", caps.summary());
    println!("models : {}", root.display());
    if let Some(n) = &caps_note {
        println!("note   : {n}");
    }
    println!();
    println!(
        "{:<2} {:<21} {:<31} {:<13} {:>10} {:>5} {:<10} {:<8} {:>8} {:<9}",
        "", "ID", "NAME", "ENGINE", "DOWNLOAD", "LANGS", "SPEED", "QUALITY", "EST RTF", "INSTALLED"
    );
    for (r, p) in recs.iter().zip(&paths) {
        let e = r.entry;
        let mark = if Some(e.id.as_str()) == recommended_id.as_deref() {
            "*"
        } else {
            ""
        };
        let est = match r.estimated_rtf {
            Some(v) => format!("~{v:.3}"),
            None => "—".to_string(),
        };
        println!(
            "{:<2} {:<21} {:<31} {:<13} {:>10} {:>5} {:<10} {:<8} {:>8} {:<9}",
            mark,
            trunc(&e.id, 21),
            trunc(&e.name, 31),
            trunc(e.engine.as_str(), 13),
            bytes_or_dash(e.download_bytes),
            e.languages.len(),
            e.speed.label(),
            e.quality.label(),
            est,
            p.state.label(),
        );
        if !r.runnable {
            for b in &r.blockers {
                println!("{:<2} {:<21} └─ {b}", "", "");
            }
        }
    }
    println!();
    if let Some(id) = &recommended_id {
        println!("*  recommended for your hardware: {id}");
    } else {
        println!("*  no catalog entry is runnable on this machine with this build");
    }
    println!("   {ESTIMATE_DISCLAIMER}");
    println!("   {MEASURE_HINT}");
    println!("   INSTALLED: yes/partial/no from the pinned manifest; n/a = no pinned file set yet.");
    Ok(())
}

/// JSON for one recommendation + its install state.
fn rec_json(r: &Recommendation<'_>, p: &EntryPaths, recommended: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "entry": r.entry,
        "runnable": r.runnable,
        "recommended": Some(r.entry.id.as_str()) == recommended,
        "best_hardware": r.best_hardware,
        "estimated_rtf": r.estimated_rtf,
        "estimated_rtf_is_an_estimate": true,
        "catalog_measurement": r.measured_reference,
        "reason": r.reason,
        "blockers": r.blockers,
        "install_state": p.state,
        "install_dir": p.dir.as_ref().map(|d| d.display().to_string()),
        "manifest_error": p.manifest_error,
    })
}

// ---------------------------------------------------------------------------------------------
// `lw models info`
// ---------------------------------------------------------------------------------------------

fn info(
    runtime_dir: &Option<PathBuf>,
    id: &str,
    json: bool,
    dest: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
    manifests_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let catalog = load_catalog(catalog_path)?;
    // `info` builds no sessions, so the strong probe is safe here.
    let (caps, caps_note, _rt) = detect_caps(runtime_dir, Probe::Enumerate);
    let root = dest.unwrap_or_else(default_models_root);
    let mdir = manifests_dir(manifests_path);

    let recs = catalog.recommend(&caps);
    let rec = recs
        .iter()
        .find(|r| r.entry.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown model id `{id}` (see `lw models list`)"))?;
    let e = rec.entry;
    let paths = entry_paths(e, mdir.as_deref(), &root, &caps);

    if json {
        println!("{}", serde_json::to_string_pretty(&rec_json(rec, &paths, None))?);
        return Ok(());
    }

    println!("{}  ({})", e.name, e.id);
    println!("{}", e.description);
    println!();
    println!("  engine        : {}", e.engine);
    println!(
        "  licence       : {}",
        if e.license.is_empty() {
            "—"
        } else {
            e.license.as_str()
        }
    );
    println!(
        "  languages     : {} ({})",
        e.languages.len(),
        e.languages.join(", ")
    );
    println!(
        "  quality tier  : {}   (editorial ranking, not a measurement)",
        e.quality
    );
    println!("  speed tier    : {}   (the input to the RTF estimate)", e.speed);
    println!("  download      : {}", bytes_or_dash(e.download_bytes));
    println!("  on disk       : {}", bytes_or_dash(e.disk_bytes));
    println!(
        "  hardware      : {}",
        e.hardware
            .iter()
            .map(|h| h.label())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(f) = &e.requires_engine_feature {
        println!("  needs feature : {f}");
    }
    match &e.manifest {
        Some(m) => println!("  manifest      : {m}"),
        None => println!("  manifest      : none — file set not hash-pinned, install is refused"),
    }
    if let Some(u) = &e.source_url {
        println!("  upstream      : {u}");
    }
    println!();
    println!("On this machine ({})", caps.summary());
    if let Some(n) = &caps_note {
        println!("  note          : {n}");
    }
    println!("  runnable      : {}", if rec.runnable { "yes" } else { "no" });
    println!(
        "  best target   : {}",
        rec.best_hardware.map(|h| h.label()).unwrap_or("—")
    );
    match rec.estimated_rtf {
        Some(v) => println!("  ESTIMATED RTF : ~{v:.4}   <-- AN ESTIMATE, NOT A MEASUREMENT"),
        None => println!("  ESTIMATED RTF : —"),
    }
    println!("  why           : {}", rec.reason);
    for b in &rec.blockers {
        println!("  blocker       : {b}");
    }
    println!(
        "  installed     : {} {}",
        paths.state.label(),
        paths
            .dir
            .as_ref()
            .map(|d| format!("({})", d.display()))
            .unwrap_or_default()
    );
    if let Some(err) = &paths.manifest_error {
        println!("  manifest note : {err}");
    }

    println!();
    if e.measurements.is_empty() {
        println!("MEASURED numbers: none in the catalog for this model.");
    } else {
        println!("MEASURED numbers (real runs — read the machine before applying them to yours):");
        for m in &e.measurements {
            let wer = m
                .wer
                .map(|w| format!("   WER {:.1}%", w * 100.0))
                .unwrap_or_default();
            println!("  [{}] RTF {:.4}{wer}", m.hardware.label(), m.rtf);
            println!("        machine: {}", m.machine);
            println!("        source : {}", m.source);
        }
    }
    if !e.wer_estimates.is_empty() {
        let s = e
            .wer_estimates
            .iter()
            .map(|(k, v)| format!("{k} {:.1}%", v * 100.0))
            .collect::<Vec<_>>()
            .join(", ");
        println!();
        println!("WER ESTIMATES by language (published or small-sample; not a promise): {s}");
    }
    println!();
    println!("Notes: {}", e.notes);
    println!();
    println!("{ESTIMATE_DISCLAIMER}");
    println!("{MEASURE_HINT}");
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// `lw models install`
// ---------------------------------------------------------------------------------------------

fn parse_target(s: &str) -> anyhow::Result<ArtifactTarget> {
    Ok(match s {
        "any" => ArtifactTarget::Any,
        "cpu_int8" | "cpu" => ArtifactTarget::CpuInt8,
        "qnn_htp_v73" | "v73" => ArtifactTarget::QnnHtpV73,
        "qnn_htp_v81" | "v81" => ArtifactTarget::QnnHtpV81,
        "core_ml" | "coreml" => ArtifactTarget::CoreMl,
        other => anyhow::bail!("unknown target `{other}` (any, cpu_int8, qnn_htp_v73, qnn_htp_v81, core_ml)"),
    })
}

fn install(
    runtime_dir: &Option<PathBuf>,
    id: &str,
    dest: &Path,
    target: Option<&str>,
    catalog_path: Option<PathBuf>,
    manifests_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let catalog = load_catalog(catalog_path)?;
    let entry = catalog
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("unknown model id `{id}` (see `lw models list`)"))?;

    let Some(manifest_name) = &entry.manifest else {
        anyhow::bail!(
            "`{id}` has no pinned manifest, so there is nothing safe to download.\n\
             Its file set has not been hash-pinned yet (see `lw models info {id}`); LocalWisper \
             refuses to fetch unverified model files. A manifest is added once the files are \
             downloaded, hashed and committed under models/manifests/."
        );
    };
    let Some(mdir) = manifests_dir(manifests_path) else {
        anyhow::bail!(
            "could not locate models/manifests/ — pass --manifests <dir> pointing at the directory \
             containing {manifest_name}"
        );
    };
    let manifest_path = mdir.join(manifest_name);
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| anyhow::anyhow!("{}: {e}", manifest_path.display()))?;
    let manifest: ModelManifest =
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", manifest_path.display()))?;
    manifest
        .validate()
        .map_err(|e| anyhow::anyhow!("{}: {e}", manifest_path.display()))?;

    let prefs = match target {
        Some(t) => vec![parse_target(t)?],
        None => preferred_targets(&detect_caps(runtime_dir, Probe::LibraryPresent).0),
    };
    let (chosen, files) = manifest.select_files(&prefs).ok_or_else(|| {
        anyhow::anyhow!(
            "{id}: no artifact set matches {} (manifest has: {})",
            prefs
                .iter()
                .map(|t| format!("{t:?}"))
                .collect::<Vec<_>>()
                .join(" > "),
            manifest
                .artifacts
                .iter()
                .map(|a| format!("{:?}", a.target))
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    let registry = ModelRegistry::new(dest);
    let final_dir = registry.model_dir(&manifest);
    let staging = staging_dir_for(&final_dir);
    let total: u64 = files.iter().map(|f| f.bytes).sum();

    println!("installing {} ({})", manifest.name, manifest.id);
    println!("  target   : {chosen:?}");
    println!("  files    : {} ({})", files.len(), bytes_or_dash(Some(total)));
    println!("  licence  : {}", manifest.license);
    println!("  into     : {}", final_dir.display());
    println!("  each file is size- and SHA-256-verified against {manifest_name} before use");
    println!();

    let bar = indicatif::ProgressBar::new(total);
    bar.set_style(
        indicatif::ProgressStyle::with_template(
            "  {msg:28} [{bar:32}] {bytes:>10}/{total_bytes:<10} {bytes_per_sec:>11} {eta:>5}",
        )
        .unwrap()
        .progress_chars("=> "),
    );

    let downloader = ModelDownloader::new().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let cancel = CancellationToken::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // Bytes fully downloaded before the current file, so the bar tracks the whole install.
    let sizes: Vec<u64> = files.iter().map(|f| f.bytes).collect();
    let result = rt.block_on(downloader.install(&files, &staging, &final_dir, cancel, |ev| {
        use lw_core::model::DownloadEvent as E;
        match ev {
            E::FileStarted { path, .. } => {
                bar.set_message(trunc(&path, 28));
            }
            E::Progress(p) => {
                let done_before: u64 = sizes[..p.file_index].iter().sum();
                bar.set_position(done_before + p.file_downloaded);
            }
            E::FileVerified { path } => {
                bar.println(format!("  verified {path}"));
            }
            E::Completed => {
                bar.finish_and_clear();
            }
        }
    }));
    bar.finish_and_clear();
    result.map_err(|e| anyhow::anyhow!(e.to_string()))?;

    println!("installed to {}", final_dir.display());
    println!(
        "measure it: lw bench --quick --model-dir \"{}\"",
        final_dir.display()
    );
    Ok(())
}

/// Arguments for `lw bench --quick`.
pub struct QuickArgs {
    /// Runtime directory override.
    pub runtime_dir: Option<PathBuf>,
    /// Model directory to load.
    pub model_dir: PathBuf,
    /// Cache directory (QNN context binaries).
    pub cache_dir: PathBuf,
    /// Backend to force.
    pub backend: lw_engine_parakeet::BackendKind,
    /// CPU threads (0 = default).
    pub threads: usize,
    /// Explicit fixtures directory, if the user gave one.
    pub fixtures: Option<PathBuf>,
}

/// `lw bench --quick`: measure THIS machine in a few seconds, with per-stage timings.
///
/// Everything printed here is measured now, on this machine. No estimates appear.
pub fn bench_quick(args: QuickArgs) -> anyhow::Result<()> {
    println!("lw bench --quick — every number below is MEASURED on this machine, just now.");

    let t_rt = Instant::now();
    let rt = crate::init_runtime(&args.runtime_dir)?;
    let ort_ms = t_rt.elapsed().as_secs_f32() * 1000.0;

    // Non-invasive: registering the QNN EP here would pull it into the CPU sessions the engine is
    // about to build. The engine decides; `backend` below reports where it really ended up.
    let mut caps = lw_platform::caps::detect();
    caps.providers.cpu = true;
    caps.providers.qnn = caps.npu.present && rt.qnn_available();
    println!("machine   : {}", caps.summary());
    println!();

    let t_load = Instant::now();
    let mut engine = crate::build_engine(rt, &args.model_dir, &args.cache_dir, args.backend, args.threads)?;
    let load_ms = t_load.elapsed().as_secs_f32() * 1000.0;

    let health = engine.health_check();
    let (clips, source) = quick_clips(args.fixtures, 3)?;
    let total_audio: f32 = clips.iter().map(|c| c.duration_s).sum();

    println!("backend   : {} on {}", engine.provider(), engine.device().name);
    println!(
        "audio     : {} — {} clip(s), {total_audio:.2} s",
        source.describe(),
        clips.len()
    );
    println!();
    println!("{:<30} {:>10}", "stage (measured)", "time");
    println!("{:<30} {:>8.0} ms", "ort runtime init", ort_ms);
    println!("{:<30} {:>8.0} ms", "engine load (sessions)", load_ms);
    match health.probe_latency_ms {
        Some(ms) => println!(
            "{:<30} {:>8.0} ms   provider: {}",
            "health probe", ms, health.provider
        ),
        None => println!("{:<30} {:>10}   {}", "health probe", "failed", health.message),
    }

    let m = measure(&mut engine, &clips, source, |c: &ClipResult| {
        let w = c.wer.map(|w| format!("   WER {w:.2}")).unwrap_or_default();
        println!(
            "{:<30} {:>8.0} ms   RTF {:.4}{w}",
            format!("transcribe {}", c.name),
            c.ms,
            c.rtf
        );
    })
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    println!();
    println!("MEASURED RTF (first/cold run) : {:.4}", m.cold_rtf);
    if m.warm_ms.is_empty() {
        println!("MEASURED RTF (warm)           : n/a (only one clip)");
    } else {
        let min = m.warm_ms.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = m.warm_ms.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        println!(
            "MEASURED RTF (warm mean)      : {:.4}   over {} run(s), {min:.0}–{max:.0} ms",
            m.warm_rtf,
            m.warm_ms.len()
        );
    }
    match m.wer {
        Some(w) => println!(
            "MEASURED WER (word-weighted)  : {w:.3}   over {} clip(s)",
            m.results.len()
        ),
        None => println!("MEASURED WER                  : n/a — synthetic audio has no reference text"),
    }
    if m.source == ClipSource::Synthetic {
        println!();
        println!(
            "CAVEAT: the audio was synthesized, not spoken. The TDT decoder emits far fewer tokens\n\
             than on real speech, so this RTF is a LOWER BOUND — real dictation will be slower.\n\
             Point --fixtures at a directory with WAVs + fixtures.json for a representative number."
        );
    }
    println!();
    println!("Full fixture benchmark (RTF + WER over the whole set): lw bench <fixtures-dir> --model-dir …");
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// `lw models compare`
// ---------------------------------------------------------------------------------------------

struct CompareArgs {
    runtime_dir: Option<PathBuf>,
    dest: Option<PathBuf>,
    fixtures: Option<PathBuf>,
    backend: BackendArg,
    no_run: bool,
    json: bool,
    catalog: Option<PathBuf>,
    manifests: Option<PathBuf>,
}

/// One row of the comparison table.
///
/// `estimated_hardware` and `measured_hardware` are kept apart on purpose: forcing `--backend cpu`
/// on a machine whose best target is the NPU produces a CPU measurement that must not be read as
/// confirming (or refuting) an NPU estimate.
struct CompareRow {
    id: String,
    estimated_hardware: Option<HardwareTarget>,
    estimated_rtf: Option<f32>,
    measured_hardware: Option<HardwareTarget>,
    measured_rtf: Option<f32>,
    measured_wer: Option<f32>,
    measured_with: Option<String>,
    status: String,
}

fn compare(args: CompareArgs) -> anyhow::Result<()> {
    let catalog = load_catalog(args.catalog)?;
    // `compare` goes on to build engines, so it must NOT register the QNN EP behind their back,
    // and must hand them the very same runtime handle.
    let (caps, caps_note, runtime) = detect_caps(&args.runtime_dir, Probe::LibraryPresent);
    let root = args.dest.unwrap_or_else(default_models_root);
    let mdir = manifests_dir(args.manifests);
    let recs = catalog.recommend(&caps);

    let mut rows = Vec::new();
    for r in &recs {
        let e = r.entry;
        let paths = entry_paths(e, mdir.as_deref(), &root, &caps);
        let mut row = CompareRow {
            id: e.id.clone(),
            estimated_hardware: r.best_hardware,
            estimated_rtf: r.estimated_rtf,
            measured_hardware: None,
            measured_rtf: None,
            measured_wer: None,
            measured_with: None,
            status: String::new(),
        };

        // Only Parakeet has an engine in this build, and only an installed model can be run.
        let can_measure = r.runnable
            && e.engine == EngineKind::ParakeetTdt
            && paths.state == InstallState::Installed
            && paths.dir.is_some();

        if args.no_run {
            row.status = "not run (--no-run)".into();
        } else if !r.runnable {
            row.status = r.blockers.first().cloned().unwrap_or_else(|| r.reason.clone());
        } else if paths.state != InstallState::Installed {
            row.status = match paths.state {
                InstallState::Unpinned => "no pinned manifest — cannot install or measure".into(),
                InstallState::Incomplete => "install incomplete".into(),
                _ => "not installed".into(),
            };
        } else if e.engine != EngineKind::ParakeetTdt {
            row.status = format!("installed, but no `{}` engine in this build", e.engine);
        }

        if can_measure && !args.no_run {
            let dir = paths.dir.clone().expect("checked above");
            eprintln!(
                "measuring {} … (loading the model; a first NPU run also prepares a context \
                 binary, which can take minutes)",
                e.id
            );
            let Some(rt) = runtime.clone() else {
                row.status = "ONNX Runtime not loaded — cannot measure".into();
                rows.push(row);
                continue;
            };
            match measure_installed(rt, &dir, args.backend, args.fixtures.clone()) {
                Ok(run) => {
                    let m = run.measurement;
                    row.measured_hardware = run.hardware;
                    row.measured_rtf = Some(m.warm_rtf);
                    row.measured_wer = m.wer;
                    row.measured_with = Some(format!(
                        "{}, {} clip(s), {:.1} s, {}",
                        run.provider,
                        m.results.len(),
                        m.audio_secs,
                        match &m.source {
                            ClipSource::Fixtures { .. } => "real speech",
                            ClipSource::Synthetic => "SYNTHETIC audio (RTF is a lower bound)",
                        }
                    ));
                    row.status = if run.hardware == row.estimated_hardware {
                        "measured just now".into()
                    } else {
                        format!(
                            "measured on {} — NOT comparable with the {} estimate",
                            run.hardware.map(|h| h.label()).unwrap_or("an unmapped target"),
                            row.estimated_hardware.map(|h| h.label()).unwrap_or("—")
                        )
                    };
                }
                Err(e) => row.status = format!("measurement failed: {e}"),
            }
        }
        rows.push(row);
    }

    if args.json {
        let out = serde_json::json!({
            "machine": caps.summary(),
            "models_root": root.display().to_string(),
            "capability_note": caps_note,
            "estimate_disclaimer": ESTIMATE_DISCLAIMER,
            "rows": rows.iter().map(|r| serde_json::json!({
                "id": r.id,
                "estimated_hardware": r.estimated_hardware,
                "estimated_rtf": r.estimated_rtf,
                "estimated_rtf_is_an_estimate": true,
                "measured_hardware": r.measured_hardware,
                "measured_rtf": r.measured_rtf,
                "measured_wer": r.measured_wer,
                "measured_with": r.measured_with,
                "status": r.status,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Model comparison — estimates and measurements side by side, never mixed.");
    println!("machine: {}", caps.summary());
    println!("models : {}", root.display());
    if let Some(n) = &caps_note {
        println!("note   : {n}");
    }
    println!();
    println!(
        "{:<22} {:<8} {:>9} {:>8} {:>9} {:>9}  STATUS",
        "ID", "EST HW", "EST RTF", "MEAS HW", "MEAS RTF", "MEAS WER"
    );
    for r in &rows {
        println!(
            "{:<22} {:<8} {:>9} {:>8} {:>9} {:>9}  {}",
            trunc(&r.id, 22),
            r.estimated_hardware.map(|h| h.label()).unwrap_or("—"),
            r.estimated_rtf
                .map(|v| format!("~{v:.4}"))
                .unwrap_or_else(|| "—".into()),
            r.measured_hardware.map(|h| h.label()).unwrap_or("—"),
            r.measured_rtf
                .map(|v| format!("{v:.4}"))
                .unwrap_or_else(|| "—".into()),
            r.measured_wer
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "—".into()),
            r.status,
        );
        if let Some(w) = &r.measured_with {
            println!("{:<22} └─ measured with: {w}", "");
        }
    }
    println!();
    println!("EST HW    the target the ESTIMATE assumes — the best one this machine offers.");
    println!("EST RTF   estimate. {ESTIMATE_DISCLAIMER}");
    println!("MEAS HW   the target the run actually executed on. When it differs from EST HW the");
    println!("          two numbers describe different hardware and must not be compared.");
    println!("MEAS RTF  MEASURED on this machine during this command (warm mean over the clips).");
    println!("MEAS WER  MEASURED word-weighted WER; blank when no reference transcripts were used.");
    println!();
    println!("Catalog reference measurements taken on other machines (not yours):");
    let mut any_ref = false;
    for e in catalog.iter() {
        for m in &e.measurements {
            any_ref = true;
            println!(
                "  {:<22} [{}] RTF {:.4}{}  on {}",
                trunc(&e.id, 22),
                m.hardware.label(),
                m.rtf,
                m.wer.map(|w| format!(" WER {w:.3}")).unwrap_or_default(),
                m.machine
            );
        }
    }
    if !any_ref {
        println!("  (none)");
    }
    Ok(())
}

/// A completed measurement together with the hardware it truly ran on.
struct MeasuredRun {
    measurement: Measurement,
    /// Where the engine *actually* executed, read back from the engine after init — not what was
    /// requested. An engine that wanted the NPU and fell back reports the CPU here. `None` when the
    /// engine ran somewhere the catalog has no target for, which is better than guessing.
    hardware: Option<HardwareTarget>,
    /// The provider string the engine reports.
    provider: String,
}

/// Map what the engine reports it is running on onto a catalog hardware target.
fn hardware_of(engine: &lw_engine_parakeet::ParakeetEngine) -> Option<HardwareTarget> {
    use lw_core::engine::Acceleration;
    match engine.acceleration() {
        Acceleration::Cpu => Some(HardwareTarget::Cpu),
        Acceleration::Npu => Some(HardwareTarget::QnnNpu),
        Acceleration::Ane => Some(HardwareTarget::CoreMl),
        // A GPU run has no catalog equivalent yet; say so rather than filing it under CPU.
        Acceleration::Gpu => None,
    }
}

/// Load an installed model and measure it.
///
/// Takes an already-initialized runtime: initializing a second `OrtRuntime` in the same process
/// leaves the QNN EP registered under the first handle, so the engine would silently lose its NPU
/// path and report a CPU number under an NPU heading.
fn measure_installed(
    rt: Arc<OrtRuntime>,
    model_dir: &Path,
    backend: BackendArg,
    fixtures: Option<PathBuf>,
) -> anyhow::Result<MeasuredRun> {
    let cache = std::env::temp_dir().join("localwisper-cache");
    let mut engine = crate::build_engine(rt, model_dir, &cache, backend.into(), 0)?;
    let provider = format!("{}", engine.provider());
    let hardware = hardware_of(&engine);
    let (clips, source) = quick_clips(fixtures, 3)?;
    let measurement =
        measure(&mut engine, &clips, source, |_| {}).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(MeasuredRun {
        measurement,
        hardware,
        provider,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_or_dash_formats() {
        assert_eq!(bytes_or_dash(None), "—");
        assert_eq!(bytes_or_dash(Some(512)), "512 B");
        assert_eq!(bytes_or_dash(Some(1 << 20)), "1.0 MiB");
        assert_eq!(bytes_or_dash(Some(670_619_803)), "639.6 MiB");
    }

    #[test]
    fn trunc_adds_ellipsis_only_when_needed() {
        assert_eq!(trunc("abc", 5), "abc");
        assert_eq!(trunc("abcdef", 4), "abc…");
    }

    #[test]
    fn parse_target_accepts_aliases_and_rejects_junk() {
        assert_eq!(parse_target("v81").unwrap(), ArtifactTarget::QnnHtpV81);
        assert_eq!(parse_target("cpu").unwrap(), ArtifactTarget::CpuInt8);
        assert!(parse_target("gpu").is_err());
    }

    #[test]
    fn preferred_targets_put_cpu_last_and_npu_first() {
        let mut caps = Capabilities::unknown();
        assert_eq!(
            preferred_targets(&caps),
            vec![ArtifactTarget::CpuInt8, ArtifactTarget::Any]
        );
        caps.npu.present = true;
        caps.providers.qnn = true;
        caps.npu.htp_arch = Some(lw_core::capabilities::HtpArch::V81);
        assert_eq!(preferred_targets(&caps)[0], ArtifactTarget::QnnHtpV81);
    }

    #[test]
    fn install_state_labels() {
        assert_eq!(InstallState::Installed.label(), "yes");
        assert_eq!(InstallState::Unpinned.label(), "n/a");
    }

    #[test]
    fn unpinned_entry_has_no_install_dir() {
        let cat = Catalog::builtin().unwrap();
        let e = cat.get("whisper-base").unwrap();
        let caps = Capabilities::unknown();
        let p = entry_paths(e, None, Path::new("/nonexistent"), &caps);
        assert_eq!(p.state, InstallState::Unpinned);
        assert!(p.dir.is_none());
    }
}
