//! `lw` — LocalWisper command-line tool.
//!
//! Subcommands:
//! - `diagnose`  — print the capability/runtime report (JSON or text).
//! - `transcribe`— transcribe a WAV file with Parakeet (CPU or NPU).
//! - `bench`     — benchmark the pipeline over a fixtures directory (RTF + optional WER), or
//!   `--quick` to measure this machine in a few seconds with no fixtures.
//! - `models`    — the model catalog: `list`, `info`, `install`, `compare`.
//! - `record` / `devices` — live microphone paths.
//! - `selfcheck` — a machine-readable readiness report (JSON).
//!
//! Honesty rule for output: a number is either **measured** (timed here, now, or carried with the
//! name of the machine it was taken on) or an **estimate**. The two are never printed in the same
//! column, and estimates are always marked. See [`models`].

mod models;

use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};

use lw_core::bench::load_wav;
use lw_core::engine::{EngineInitContext, SpeechEngine};
use lw_engine_parakeet::{BackendKind, ParakeetConfig, ParakeetEngine};
use lw_ort::OrtRuntime;

#[derive(Parser)]
#[command(
    name = "lw",
    version,
    about = "LocalWisper CLI: diagnostics, transcription, benchmarks"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Runtime directory holding onnxruntime + QNN EP DLLs (default: auto-detect).
    #[arg(long, global = true)]
    runtime_dir: Option<PathBuf>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum BackendArg {
    /// Best usable accelerator, falling back to the CPU.
    Auto,
    Cpu,
    /// Any NPU on this machine (Qualcomm, Intel, AMD).
    Npu,
    /// Any GPU on this machine (WebGPU, CUDA, TensorRT, DirectML, CoreML).
    Gpu,
    /// One exact provider, so a measurement cannot be misattributed.
    Qnn,
    Webgpu,
    Cuda,
    Tensorrt,
    Directml,
    Coreml,
    Openvino,
    Vitisai,
}

impl From<BackendArg> for BackendKind {
    fn from(b: BackendArg) -> Self {
        use lw_core::capabilities::Accelerator as A;
        match b {
            BackendArg::Auto => BackendKind::Auto,
            BackendArg::Cpu => BackendKind::ForceCpu,
            BackendArg::Npu => BackendKind::ForceNpu,
            BackendArg::Gpu => BackendKind::ForceGpu,
            BackendArg::Qnn => BackendKind::Exact(A::QnnNpu),
            BackendArg::Webgpu => BackendKind::Exact(A::WebGpu),
            BackendArg::Cuda => BackendKind::Exact(A::Cuda),
            BackendArg::Tensorrt => BackendKind::Exact(A::TensorRt),
            BackendArg::Directml => BackendKind::Exact(A::DirectMl),
            BackendArg::Coreml => BackendKind::Exact(A::CoreMl),
            BackendArg::Openvino => BackendKind::Exact(A::OpenVino),
            BackendArg::Vitisai => BackendKind::Exact(A::VitisAi),
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Print hardware/runtime diagnostics.
    Diagnose {
        /// Emit JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Transcribe a WAV file.
    Transcribe {
        /// Path to a 16-bit or float WAV file.
        wav: PathBuf,
        /// Model directory.
        #[arg(long)]
        model_dir: PathBuf,
        /// Cache directory (QNN context binaries).
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// Backend to use.
        #[arg(long, value_enum, default_value_t = BackendArg::Auto)]
        backend: BackendArg,
        /// CPU threads (0 = default).
        #[arg(long, default_value_t = 0)]
        threads: usize,
    },
    /// Benchmark over a fixtures directory containing `fixtures.json` (or `--quick`).
    Bench {
        /// Directory with WAV files and `fixtures.json`. Optional when `--quick` is given.
        fixtures: Option<PathBuf>,
        /// Model directory.
        #[arg(long)]
        model_dir: PathBuf,
        /// Cache directory.
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// Backend to use.
        #[arg(long, value_enum, default_value_t = BackendArg::Auto)]
        backend: BackendArg,
        /// CPU threads (0 = default).
        #[arg(long, default_value_t = 0)]
        threads: usize,
        /// Score only these languages, comma-separated (e.g. `--languages ru` or `en,ru`).
        ///
        /// For a model whose files carry no language metadata -- an `encoder/decoder/joiner`
        /// transducer export, say -- this is how you state what it actually handles, so the run
        /// produces one meaningful figure instead of a blend across languages it cannot speak.
        #[arg(long, value_delimiter = ',')]
        languages: Vec<String>,
        /// Few-second self-measurement with per-stage timings; needs no fixtures directory.
        ///
        /// Uses `tests/fixtures/audio` if it can find one, otherwise synthesizes speech-like
        /// audio and says so. Every number it prints is measured on this machine.
        #[arg(long)]
        quick: bool,
    },
    /// Browse, inspect, install and compare speech models.
    Models {
        /// The models subcommand.
        #[command(subcommand)]
        cmd: models::ModelsCmd,
    },
    /// Record from the microphone for N seconds, then transcribe (live end-to-end).
    Record {
        /// Model directory.
        #[arg(long)]
        model_dir: PathBuf,
        /// Cache directory.
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// Seconds to record.
        #[arg(long, default_value_t = 5.0)]
        seconds: f32,
        /// Backend to use.
        #[arg(long, value_enum, default_value_t = BackendArg::Auto)]
        backend: BackendArg,
        /// Input device name (default: system default).
        #[arg(long)]
        device: Option<String>,
    },
    /// List available microphone input devices.
    Devices,
    /// Machine-readable readiness report (JSON).
    Selfcheck,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    };
    // Not std::process::exit: see lw_ort::exit_without_teardown for the upstream WebGPU EP
    // teardown crash this avoids. Output is already flushed there.
    lw_ort::exit_without_teardown(code);
}

fn init_runtime(runtime_dir: &Option<PathBuf>) -> anyhow::Result<std::sync::Arc<OrtRuntime>> {
    let rt = match runtime_dir {
        Some(d) => OrtRuntime::init(d)?,
        None => OrtRuntime::auto()?,
    };
    Ok(rt)
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Diagnose { json } => diagnose(&cli.runtime_dir, json),
        Command::Selfcheck => selfcheck(&cli.runtime_dir),
        Command::Transcribe {
            wav,
            model_dir,
            cache_dir,
            backend,
            threads,
        } => {
            let rt = init_runtime(&cli.runtime_dir)?;
            let cache = cache_dir.unwrap_or_else(|| std::env::temp_dir().join("localwisper-cache"));
            let mut engine = build_engine(rt, &model_dir, &cache, backend.into(), threads)?;
            let audio = load_wav(&wav)?;
            let t0 = Instant::now();
            let transcript = engine
                .transcribe(&audio)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let ms = t0.elapsed().as_secs_f32() * 1000.0;
            println!("backend : {} on {}", engine.provider(), engine.device().name);
            println!(
                "audio   : {:.2}s   time: {:.0}ms   RTF: {:.3}",
                audio.duration_secs(),
                ms,
                (ms / 1000.0) / audio.duration_secs().max(1e-6)
            );
            println!("text    : {}", transcript.text);
            Ok(())
        }
        Command::Bench {
            fixtures,
            model_dir,
            cache_dir,
            backend,
            threads,
            languages,
            quick,
        } => {
            let cache = cache_dir.unwrap_or_else(|| std::env::temp_dir().join("localwisper-cache"));
            if quick {
                return models::bench_quick(models::QuickArgs {
                    runtime_dir: cli.runtime_dir.clone(),
                    model_dir,
                    cache_dir: cache,
                    backend: backend.into(),
                    threads,
                    fixtures,
                });
            }
            let fixtures = fixtures.ok_or_else(|| {
                anyhow::anyhow!(
                    "bench needs a fixtures directory containing fixtures.json — \
                     pass one, or use `lw bench --quick` to measure this machine without fixtures"
                )
            })?;
            let rt = init_runtime(&cli.runtime_dir)?;
            bench(
                rt,
                &fixtures,
                &model_dir,
                &cache,
                backend.into(),
                threads,
                &languages,
            )
        }
        Command::Models { cmd } => models::run(cmd, &cli.runtime_dir),
        Command::Devices => {
            for d in lw_platform::audio::list_input_devices() {
                println!("{d}");
            }
            Ok(())
        }
        Command::Record {
            model_dir,
            cache_dir,
            seconds,
            backend,
            device,
        } => {
            let rt = init_runtime(&cli.runtime_dir)?;
            let cache = cache_dir.unwrap_or_else(|| std::env::temp_dir().join("localwisper-cache"));
            let mut engine = build_engine(rt, &model_dir, &cache, backend.into(), 0)?;
            record_and_transcribe(engine.as_mut(), seconds, device)
        }
    }
}

/// Capture from the microphone for `seconds`, then transcribe — the live audio path.
fn record_and_transcribe(
    engine: &mut dyn SpeechEngine,
    seconds: f32,
    device: Option<String>,
) -> anyhow::Result<()> {
    use lw_platform::AudioCapture;
    let mut capture = lw_platform::Capture::new(device, 16_000 * 90);
    capture.start().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    eprintln!(
        "recording {seconds:.1}s at {} Hz … speak now",
        capture.native_sample_rate()
    );
    let start = Instant::now();
    while start.elapsed().as_secs_f32() < seconds {
        std::thread::sleep(std::time::Duration::from_millis(100));
        eprint!("\r  level: {:>5.3}   ", capture.level_rms());
    }
    eprintln!();
    let audio = capture.stop().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    eprintln!(
        "captured {:.2}s ({} samples @ {} Hz)",
        audio.duration_secs(),
        audio.len(),
        audio.sample_rate
    );
    let t0 = Instant::now();
    let transcript = engine
        .transcribe(&audio)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let ms = t0.elapsed().as_secs_f32() * 1000.0;
    println!("backend : {} on {}", engine.provider(), engine.device().name);
    println!("time    : {ms:.0}ms");
    println!("text    : {}", transcript.text);
    Ok(())
}

fn build_engine(
    rt: std::sync::Arc<OrtRuntime>,
    model_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    backend: BackendKind,
    threads: usize,
) -> anyhow::Result<Box<dyn SpeechEngine>> {
    let ctx = EngineInitContext {
        model_dir: model_dir.to_path_buf(),
        cache_dir: cache_dir.to_path_buf(),
        cpu_threads: threads,
    };

    // Pick the engine from what is actually in the directory rather than from a flag: a user who
    // points at a Whisper export should get Whisper, not an unhelpful error from the Parakeet
    // loader about a missing vocab.txt.
    if let Ok(files) = lw_engine_sherpa::detect_in_dir(model_dir, true, None) {
        // Only the message below needs the family name, and that branch is compiled out of a
        // sherpa build -- binding it unconditionally warns in exactly the configuration we ship.
        #[cfg(not(feature = "sherpa"))]
        let kind = files.kind();
        #[cfg(feature = "sherpa")]
        let _ = &files;
        #[cfg(feature = "sherpa")]
        {
            let cfg = lw_engine_sherpa::SherpaConfig::new(model_dir).with_threads(threads);
            let mut engine = lw_engine_sherpa::SherpaEngine::new(cfg);
            engine
                .initialize(&ctx)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            return Ok(Box::new(engine));
        }
        #[cfg(not(feature = "sherpa"))]
        {
            anyhow::bail!(
                "{} holds a {kind} model, which needs a build with the `sherpa` engine feature.
                 Build it with:
                   $env:SHERPA_ONNX_LIB_DIR = (pwsh -File scripts/build/fetch-sherpa.ps1 -Quiet)
                   cargo build --release -p lw-cli --features sherpa
                 See docs/build.md -- the extra step keeps GPL-3.0 code out of the binary.",
                model_dir.display()
            );
        }
    }

    let config = ParakeetConfig::from_ctx(&ctx, backend);
    // Take the Hexagon generation from the detected hardware: X Elite / X Plus are V73, X2 Elite
    // is V81, and a context binary prepared for one will not load on the other.
    let config = config.with_capabilities(&lw_platform::caps::detect());
    let mut engine = ParakeetEngine::new(rt, config);
    engine
        .initialize(&ctx)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(Box::new(engine))
}

fn diagnose(runtime_dir: &Option<PathBuf>, json: bool) -> anyhow::Result<()> {
    let mut report = serde_json::Map::new();
    let mut accelerators: Vec<lw_ort::AcceleratorStatus> = Vec::new();
    report.insert("app_version".into(), env!("CARGO_PKG_VERSION").into());
    report.insert("os".into(), std::env::consts::OS.into());
    report.insert("arch".into(), std::env::consts::ARCH.into());

    match init_runtime(runtime_dir) {
        Ok(rt) => {
            let has_npu = rt.has_qnn_npu(); // registers the QNN EP to enumerate devices
            report.insert(
                "runtime_dir".into(),
                rt.runtime_dir().display().to_string().into(),
            );
            report.insert("qnn_registered".into(), rt.qnn_registered().into());
            report.insert("qnn_npu".into(), has_npu.into());
            report.insert("qnn_npu_count".into(), (rt.qnn_npu_count() as u64).into());
            report.insert(
                "devices".into(),
                serde_json::Value::Array(rt.device_summary().into_iter().map(Into::into).collect()),
            );
            let usable = rt.usable_accelerators();
            accelerators = rt.probe_accelerators();
            report.insert(
                "accelerators".into(),
                serde_json::to_value(&accelerators).unwrap_or_default(),
            );
            // The recommendation is the first usable accelerator in preference order, which is
            // exactly what `--backend auto` will choose -- not a guess from DLL presence.
            report.insert(
                "recommended_backend".into(),
                usable
                    .first()
                    .map(|a| a.id().to_string())
                    .unwrap_or_else(|| "cpu".to_string())
                    .into(),
            );
        }
        Err(e) => {
            report.insert("runtime_error".into(), e.to_string().into());
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("LocalWisper diagnostics");
        // The accelerator table is the useful part; print it separately rather than as raw JSON.
        report.remove("accelerators");
        if !accelerators.is_empty() {
            println!(
                "
  accelerators on this machine"
            );
            println!(
                "    {:<30} {:<4} {:<8} {:<11} {:>7}  STATUS",
                "PROVIDER", "KIND", "PRESENT", "REGISTERED", "DEVICES"
            );
            for st in &accelerators {
                println!(
                    "    {:<30} {:<4} {:<8} {:<11} {:>7}  {}",
                    st.accel.label(),
                    st.accel.kind().label(),
                    st.present,
                    st.registered,
                    st.devices,
                    st.explain()
                );
            }
            println!(
                "
  `usable` means a device enumerated -- present and registered do not.
"
            );
        }
        for (k, v) in &report {
            println!("  {k:20}: {v}");
        }
    }
    Ok(())
}

fn selfcheck(runtime_dir: &Option<PathBuf>) -> anyhow::Result<()> {
    let mut ok = true;
    let mut obj = serde_json::Map::new();
    obj.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    obj.insert("arch".into(), std::env::consts::ARCH.into());
    match init_runtime(runtime_dir) {
        Ok(rt) => {
            obj.insert("runtime".into(), "ok".into());
            obj.insert("qnn_npu".into(), rt.has_qnn_npu().into());
        }
        Err(e) => {
            ok = false;
            obj.insert("runtime".into(), format!("error: {e}").into());
        }
    }
    obj.insert("status".into(), if ok { "ok" } else { "degraded" }.into());
    println!("{}", serde_json::to_string_pretty(&obj)?);
    Ok(())
}

fn bench(
    rt: std::sync::Arc<OrtRuntime>,
    fixtures: &std::path::Path,
    model_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    backend: BackendKind,
    threads: usize,
    languages: &[String],
) -> anyhow::Result<()> {
    use lw_core::bench::{ClipResult, ClipSource, fixture_clips, measure};

    // usize::MAX: take the whole fixture set, not the 3-clip sample `--quick` uses.
    let mut clips = fixture_clips(fixtures, usize::MAX).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if !languages.is_empty() {
        let want: Vec<String> = languages
            .iter()
            .map(|l| l.trim().to_ascii_lowercase())
            .filter(|l| !l.is_empty())
            .collect();
        clips.retain(|c| {
            c.language
                .as_ref()
                .is_some_and(|l| want.contains(&l.to_ascii_lowercase()))
        });
        if clips.is_empty() {
            anyhow::bail!(
                "no clips in {} for language(s) {}",
                fixtures.display(),
                want.join("/")
            );
        }
        println!("languages: restricted to {} by --languages", want.join("/"));
    }
    if clips.is_empty() {
        anyhow::bail!("no clips in {}", fixtures.display());
    }

    let mut engine = build_engine(rt, model_dir, cache_dir, backend, threads)?;
    println!("backend: {} on {}", engine.provider(), engine.device().name);
    // "err" rather than "WER": the column can hold either a word rate or a character rate, and
    // each row says which. A fixed WER header would mislabel every Chinese line under it.
    println!(
        "{:<22} {:>7} {:>8} {:>7}  err",
        "file", "dur(s)", "time(ms)", "RTF"
    );

    let m = measure(
        engine.as_mut(),
        &clips,
        ClipSource::Fixtures {
            dir: fixtures.to_path_buf(),
        },
        |c: &ClipResult| {
            println!(
                "{:<22} {:>7.2} {:>8.0} {:>7.3}  {} {:.2} [{}]{}",
                c.name,
                c.duration_s,
                c.ms,
                c.rtf,
                c.unit.label(),
                c.wer.unwrap_or(0.0),
                c.language.as_deref().unwrap_or("?"),
                if c.scored { "" } else { " (not scored)" }
            );
        },
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    println!("---");

    // Per-language first: it is the only view that stays honest whatever the model claims.
    if m.per_language.len() > 1 {
        println!("per language:");
        for l in &m.per_language {
            println!(
                "  {:<4} {} {:.3}   over {} clip(s), {} {}{}",
                l.language,
                l.unit.label(),
                l.wer,
                l.clips,
                l.words,
                match l.unit {
                    lw_core::bench::ErrorUnit::Word => "word(s)",
                    lw_core::bench::ErrorUnit::Character => "character(s)",
                },
                if l.claimed {
                    ""
                } else {
                    "   (language not claimed by this model)"
                }
            );
        }
    }

    let scored: Vec<&ClipResult> = m.results.iter().filter(|r| r.scored).collect();
    let scored_rtf: f64 = scored.iter().map(|r| r.rtf as f64).sum();
    let all_rtf: f64 = m.results.iter().map(|r| r.rtf as f64).sum();

    if !m.engine_claimed_languages && m.per_language.len() > 1 {
        // The model told us nothing about its languages, and the clips span several. Blending
        // them into one figure would invent a claim on the model's behalf -- a Russian-only
        // transducer scored this way reads 0.78 when it is 0.00 on the Russian clips.
        println!(
            "mean RTF: {:.4}   over {} clip(s)",
            all_rtf / m.results.len().max(1) as f64,
            m.results.len()
        );
        println!(
            "WER: no single figure - this model declares no languages, and the clips span {}.",
            m.per_language
                .iter()
                .map(|l| l.language.as_str())
                .collect::<Vec<_>>()
                .join("/")
        );
        println!("     Read the per-language breakdown above, or pass --languages to pick a subset.");
        return Ok(());
    }

    // A run spanning, say, Russian and Chinese has no single total: averaging a word rate with a
    // character rate produces a number with no unit. The breakdown above is the answer.
    if m.mixed_units {
        println!(
            "mean RTF: {:.4}   over {} clip(s) in {}",
            scored_rtf / scored.len().max(1) as f64,
            scored.len(),
            m.scored_languages.join("/")
        );
        println!(
            "no single accuracy figure: these clips are scored in different units (words for some              languages, characters for the ones written without spaces), and the two cannot be              averaged. Read the per-language breakdown above."
        );
        return Ok(());
    }

    println!(
        "mean RTF: {:.4}   token-weighted {}: {:.3}   over {} clip(s) in {}",
        scored_rtf / scored.len().max(1) as f64,
        m.unit.map_or("WER", lw_core::bench::ErrorUnit::label),
        m.wer.unwrap_or(0.0),
        scored.len(),
        if m.scored_languages.is_empty() {
            "no declared language".to_string()
        } else {
            m.scored_languages.join("/")
        }
    );
    if !m.unscored_languages.is_empty() {
        let unscored = m.results.len() - scored.len();
        println!(
            "note: {unscored} clip(s) in {} were transcribed but NOT scored.",
            m.unscored_languages.join("/")
        );
        println!("      This model does not claim those languages; a WER against them would describe");
        println!("      nothing useful. Its speed on them is still shown above.");
        println!(
            "      mean RTF over all {} clip(s), scored or not: {:.4}",
            m.results.len(),
            all_rtf / m.results.len().max(1) as f64
        );
    }
    Ok(())
}
