//! Keep each optional provider runtime isolated from the process-global bundled ORT.
//! ORT's dynamic API is process-global, so two different core DLLs cannot be selected per session.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;

use lw_core::audio::AudioBuffer;
use lw_core::capabilities::Accelerator;
use lw_core::engine::{
    Acceleration, DeviceInfo, EngineInitContext, HealthReport, Language, Provider, SpeechEngine, Transcript,
};
use lw_engine_parakeet::{BackendKind, LANGUAGES, ParakeetConfig, ParakeetEngine};
use lw_ort::OrtRuntime;
use serde::{Deserialize, Serialize};

const HEALTH: u8 = 1;
const TRANSCRIBE: u8 = 2;
const SHUTDOWN: u8 = 3;

/// The self-contained Windows ML runtime staged beside the ordinary CUDA runtime.
pub fn directml_runtime_dir(primary: &Path) -> PathBuf {
    primary.with_file_name("win-x64-directml")
}

/// Whether the second process has the three binaries it needs to start.
pub fn directml_runtime_present(primary: &Path) -> bool {
    let dir = directml_runtime_dir(primary);
    [
        "onnxruntime.dll",
        "DirectML.dll",
        "Microsoft.Windows.AI.MachineLearning.dll",
    ]
    .iter()
    .all(|file| dir.join(file).is_file())
}

/// Probe the alternate runtime in a clean process, without disturbing the CUDA runtime here.
pub fn probe_directml(primary: &Path) -> Result<usize, String> {
    if !directml_runtime_present(primary) {
        return Err("DirectML runtime is not installed".into());
    }
    probe_runtime(&directml_runtime_dir(primary), Accelerator::DirectMl)
}

/// Probe a selected add-on in a clean process, without replacing any loaded DLLs.
pub fn probe_runtime(runtime_dir: &Path, accel: Accelerator) -> Result<usize, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = Command::new(exe);
    command.arg("--provider-probe").arg(runtime_dir).arg(accel.id());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{} probe timed out", accel.label()));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let answer = serde_json::from_slice::<Result<usize, String>>(&output.stdout)
        .map_err(|e| format!("{} probe did not answer: {e}", accel.label()))?;
    answer.and_then(|count| {
        if count > 0 {
            Ok(count)
        } else {
            Err(format!(
                "{} enumerated no compatible device; check the display driver",
                accel.label()
            ))
        }
    })
}

/// Called by `--provider-probe` in the isolated GUI child.
pub fn run_provider_probe(runtime_dir: &Path, accel: Accelerator) -> Result<usize, String> {
    let runtime = OrtRuntime::init(runtime_dir).map_err(|e| e.to_string())?;
    Ok(runtime.device_count(accel))
}

#[derive(Serialize, Deserialize)]
struct Ready {
    device: DeviceInfo,
    accelerator: Option<Accelerator>,
    notes: Vec<String>,
}

struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Worker {
    fn response<T: serde::de::DeserializeOwned>(&mut self) -> Result<T, String> {
        let mut line = String::new();
        if self.output.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("Provider worker closed its output".into());
        }
        serde_json::from_str::<Result<T, String>>(&line).map_err(|e| e.to_string())?
    }
}

/// A Parakeet engine whose ONNX sessions live in the Provider worker process.
pub struct ProviderEngine {
    accel: Accelerator,
    runtime_dir: PathBuf,
    worker: Option<Worker>,
    device: DeviceInfo,
    selected: Option<Accelerator>,
    notes: Vec<String>,
}

impl ProviderEngine {
    pub fn new(runtime_dir: PathBuf, accel: Accelerator) -> Self {
        Self {
            accel,
            runtime_dir,
            worker: None,
            device: DeviceInfo::new(accel.label()),
            selected: None,
            notes: Vec::new(),
        }
    }

    fn worker(&mut self) -> lw_core::Result<&mut Worker> {
        self.worker
            .as_mut()
            .ok_or_else(|| lw_core::Error::Unavailable("Provider worker has not started".into()))
    }
}

impl SpeechEngine for ProviderEngine {
    fn backend_name(&self) -> &str {
        "parakeet-tdt-0.6b-v3"
    }
    fn provider(&self) -> Provider {
        match self.accel {
            Accelerator::DirectMl => Provider::DirectMl,
            Accelerator::Cuda => Provider::Cuda,
            Accelerator::TensorRt => Provider::TensorRt,
            Accelerator::CoreMl => Provider::CoreMl,
            _ => Provider::Gpu,
        }
    }
    fn device(&self) -> DeviceInfo {
        self.device.clone()
    }
    fn notes(&self) -> &[String] {
        &self.notes
    }
    fn acceleration(&self) -> Acceleration {
        if self.accel == Accelerator::CoreMl {
            Acceleration::Ane
        } else {
            Acceleration::Gpu
        }
    }
    fn supported_languages(&self) -> &[Language] {
        LANGUAGES
    }
    fn supports_streaming(&self) -> bool {
        false
    }

    fn initialize(&mut self, ctx: &EngineInitContext) -> lw_core::Result<()> {
        if self.worker.is_some() {
            return Ok(());
        }
        if !self.runtime_dir.join(lw_ort::onnxruntime_lib_name()).is_file() {
            return Err(lw_core::Error::Unavailable(format!(
                "{} runtime is missing from {}",
                self.accel.label(),
                self.runtime_dir.display()
            )));
        }
        let exe = std::env::current_exe().map_err(lw_core::Error::other)?;
        let mut command = Command::new(exe);
        command
            .arg("--provider-worker")
            .arg(&self.runtime_dir)
            .arg(&ctx.model_dir)
            .arg(&ctx.cache_dir)
            .arg(ctx.cpu_threads.to_string())
            .arg(self.accel.id())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().map_err(lw_core::Error::other)?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| lw_core::Error::other("worker stdin missing"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| lw_core::Error::other("worker stdout missing"))?;
        let mut worker = Worker {
            child,
            input,
            output: BufReader::new(output),
        };
        let ready: Ready = worker.response().map_err(lw_core::Error::other)?;
        if ready.accelerator != Some(self.accel) {
            return Err(lw_core::Error::Unavailable(format!(
                "Provider worker did not select {}",
                self.accel.label()
            )));
        }
        self.device = ready.device;
        self.selected = ready.accelerator;
        self.notes = ready.notes;
        self.worker = Some(worker);
        Ok(())
    }

    fn health_check(&mut self) -> HealthReport {
        let result = self.worker().and_then(|worker| {
            worker.input.write_all(&[HEALTH]).map_err(lw_core::Error::other)?;
            worker.input.flush().map_err(lw_core::Error::other)?;
            worker.response().map_err(lw_core::Error::other)
        });
        result.unwrap_or_else(|error| HealthReport {
            ok: false,
            provider: self.provider(),
            probe_latency_ms: None,
            message: error.to_string(),
        })
    }

    fn transcribe(&mut self, audio: &AudioBuffer) -> lw_core::Result<Transcript> {
        let count = u32::try_from(audio.samples.len())
            .map_err(|_| lw_core::Error::Audio("audio is too long for the worker".into()))?;
        let worker = self.worker()?;
        worker
            .input
            .write_all(&[TRANSCRIBE])
            .map_err(lw_core::Error::other)?;
        worker
            .input
            .write_all(&count.to_le_bytes())
            .map_err(lw_core::Error::other)?;
        worker
            .input
            .write_all(&audio.sample_rate.to_le_bytes())
            .map_err(lw_core::Error::other)?;
        for chunk in audio.samples.chunks(4096) {
            let bytes: Vec<u8> = chunk.iter().flat_map(|sample| sample.to_le_bytes()).collect();
            worker.input.write_all(&bytes).map_err(lw_core::Error::other)?;
        }
        worker.input.flush().map_err(lw_core::Error::other)?;
        worker.response().map_err(lw_core::Error::other)
    }

    fn accelerator(&self) -> Option<Accelerator> {
        self.selected
    }

    fn shutdown(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            let _ = worker.input.write_all(&[SHUTDOWN]);
            let _ = worker.input.flush();
            let _ = worker.child.wait();
        }
    }
}

impl Drop for ProviderEngine {
    fn drop(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            let _ = worker.child.kill();
            let _ = worker.child.wait();
        }
    }
}

/// Entry point used only by the GUI executable's private worker mode.
pub fn run_provider_worker() -> Result<(), String> {
    let mut args = std::env::args_os().skip(2);
    let runtime_dir = PathBuf::from(args.next().ok_or("missing runtime directory")?);
    let model_dir = PathBuf::from(args.next().ok_or("missing model directory")?);
    let cache_dir = PathBuf::from(args.next().ok_or("missing cache directory")?);
    let cpu_threads = args
        .next()
        .ok_or("missing CPU thread count")?
        .to_string_lossy()
        .parse::<usize>()
        .map_err(|e| e.to_string())?;
    let accel = args
        .next()
        .map(|s| Accelerator::from_id(&s.to_string_lossy()).ok_or("invalid worker accelerator"))
        .transpose()?
        .unwrap_or(Accelerator::DirectMl);
    let output = std::io::stdout();
    let mut output = output.lock();
    let init = (|| {
        let runtime = OrtRuntime::init(&runtime_dir).map_err(|e| e.to_string())?;
        let ctx = EngineInitContext {
            model_dir,
            cache_dir,
            cpu_threads,
        };
        let config = ParakeetConfig::from_ctx(&ctx, BackendKind::Exact(accel));
        let config = config.with_capabilities(crate::machine::probe_capabilities());
        let mut engine = ParakeetEngine::new(Arc::clone(&runtime), config);
        engine.initialize(&ctx).map_err(|e| e.to_string())?;
        Ok::<_, String>(engine)
    })();
    let mut engine = match init {
        Ok(engine) => {
            let ready = Ready {
                device: engine.device(),
                accelerator: engine.accelerator(),
                notes: engine.notes().to_vec(),
            };
            write_response(&mut output, &Ok::<_, String>(ready))?;
            engine
        }
        Err(error) => {
            write_response(&mut output, &Err::<Ready, _>(error.clone()))?;
            return Err(error);
        }
    };
    let input = std::io::stdin();
    let mut input = input.lock();
    loop {
        let mut command = [0u8; 1];
        if input.read_exact(&mut command).is_err() {
            break;
        }
        match command[0] {
            HEALTH => write_response(&mut output, &Ok::<_, String>(engine.health_check()))?,
            TRANSCRIBE => {
                let mut header = [0u8; 8];
                input.read_exact(&mut header).map_err(|e| e.to_string())?;
                let count = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
                let sample_rate = u32::from_le_bytes(header[4..].try_into().unwrap());
                if count > 16_000 * 60 * 6 {
                    return Err("worker audio limit exceeded".into());
                }
                let mut bytes = vec![0u8; count * 4];
                input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                let samples = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                let result = engine
                    .transcribe(&AudioBuffer::new(samples, sample_rate))
                    .map_err(|e| e.to_string());
                write_response(&mut output, &result)?;
            }
            SHUTDOWN => break,
            other => return Err(format!("unknown worker command {other}")),
        }
    }
    Ok(())
}

fn write_response<T: Serialize>(output: &mut impl Write, value: &Result<T, String>) -> Result<(), String> {
    serde_json::to_writer(&mut *output, value).map_err(|e| e.to_string())?;
    output.write_all(b"\n").map_err(|e| e.to_string())?;
    output.flush().map_err(|e| e.to_string())
}
