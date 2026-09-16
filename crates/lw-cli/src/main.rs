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

use lw_core::bench::{load_wav, word_error_rate};
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
            bench(rt, &fixtures, &model_dir, &cache, backend.into(), threads)
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
        let kind = files.kind();
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
) -> anyhow::Result<()> {
    #[derive(serde::Deserialize)]
    struct Fixture {
        file: String,
        language: String,
        duration_s: f32,
        transcript: String,
    }
    let manifest = fixtures.join("fixtures.json");
    let data = std::fs::read_to_string(&manifest)?;
    let items: Vec<Fixture> = serde_json::from_str(&data)?;

    let mut engine = build_engine(rt, model_dir, cache_dir, backend, threads)?;
    println!("backend: {} on {}", engine.provider(), engine.device().name);
    println!(
        "{:<22} {:>7} {:>8} {:>7}  WER",
        "file", "dur(s)", "time(ms)", "RTF"
    );
    let mut total_err = 0.0f64;
    let mut total_words = 0usize;
    let mut total_rtf = 0.0f64;
    for it in &items {
        let audio = load_wav(&fixtures.join(&it.file))?;
        let t0 = Instant::now();
        let transcript = engine
            .transcribe(&audio)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        let rtf = (ms / 1000.0) / it.duration_s.max(1e-6);
        let (wer, nwords) = word_error_rate(&it.transcript, &transcript.text);
        total_err += wer as f64 * nwords as f64;
        total_words += nwords;
        total_rtf += rtf as f64;
        println!(
            "{:<22} {:>7.2} {:>8.0} {:>7.3}  {:.2} [{}]",
            it.file, it.duration_s, ms, rtf, wer, it.language
        );
    }
    let avg_wer = if total_words > 0 {
        total_err / total_words as f64
    } else {
        0.0
    };
    println!("---");
    println!(
        "mean RTF: {:.4}   word-weighted WER: {:.3}",
        total_rtf / items.len().max(1) as f64,
        avg_wer
    );
    Ok(())
}
