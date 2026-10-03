//! Runtime discovery + one-time ONNX Runtime initialization + QNN EP registration.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use lw_core::capabilities::{ALL_ACCELERATORS, Accelerator, AcceleratorKind};
use ort::environment::Environment;
use ort::ep::ExecutionProvider;
use ort::ep::ExecutionProviderLibrary;
use ort::memory::DeviceType;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

/// Errors from the ORT layer.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The onnxruntime dynamic library could not be located.
    #[error("onnxruntime library not found (searched: {0})")]
    NotFound(String),
    /// `ort` failed to load or initialize.
    #[error("ort init failed: {0}")]
    Init(String),
    /// The QNN plugin EP could not be registered.
    #[error("qnn registration failed: {0}")]
    Qnn(String),
    /// An accelerator was asked for that this machine or this install cannot provide.
    /// The message already names the accelerator and the reason, so it carries no prefix.
    #[error("{0}")]
    Unsupported(String),
    /// A generic error.
    #[error("{0}")]
    Other(String),
}

/// The library file name for the ONNX Runtime on this platform.
pub fn onnxruntime_lib_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "onnxruntime.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "libonnxruntime.dylib"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "libonnxruntime.so"
    }
}

/// The per-platform runtime subdirectory name used by the repo layout and the installers
/// (e.g. `win-arm64`), so a single `runtime/` tree can carry several architectures.
pub fn runtime_platform_dir() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => "win-arm64",
        ("windows", _) => "win-x64",
        ("macos", "aarch64") => "osx-arm64",
        ("macos", _) => "osx-x64",
        (_, "aarch64") => "linux-arm64",
        _ => "linux-x64",
    }
}

/// Search for a directory containing `onnxruntime`. Order:
/// 1. `LW_RUNTIME_DIR` env var,
/// 2. `ORT_DYLIB_PATH` env var (its parent directory),
/// 3. the executable's directory and its ancestors, then `runtime/<platform>/` and `runtime/`,
/// 4. the current working directory (and the same two subdirs).
///
/// Walking ancestors makes a `target/<profile>/owlwhisp` developer build runnable by double
/// click: its staged runtime normally lives at the repository root, not beside the executable.
pub fn locate_runtime_dir() -> Option<PathBuf> {
    let explicit_dir = std::env::var_os("LW_RUNTIME_DIR").map(PathBuf::from);
    let dylib_parent = std::env::var_os("ORT_DYLIB_PATH")
        .map(PathBuf::from)
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    let cwd = std::env::current_dir().ok();

    let candidates = runtime_candidates(
        explicit_dir.as_deref(),
        dylib_parent.as_deref(),
        exe_dir.as_deref(),
        cwd.as_deref(),
        runtime_platform_dir(),
    );
    let lib = onnxruntime_lib_name();
    candidates.into_iter().find(|d| d.join(lib).exists())
}

/// Build the ordered candidate list. Split out from [`locate_runtime_dir`] so the search order is
/// testable without mutating process-global state (cwd / env).
fn runtime_candidates(
    explicit_dir: Option<&Path>,
    dylib_parent: Option<&Path>,
    exe_dir: Option<&Path>,
    cwd: Option<&Path>,
    platform: &str,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.extend(explicit_dir.map(Path::to_path_buf));
    candidates.extend(dylib_parent.map(Path::to_path_buf));
    let exe_bases = exe_dir.into_iter().flat_map(|dir| {
        std::iter::successors(Some(dir.to_path_buf()), |current| {
            current.parent().map(Path::to_path_buf)
        })
    });
    let cwd_bases = cwd.into_iter().map(Path::to_path_buf);
    for base in exe_bases.chain(cwd_bases) {
        candidates.push(base.to_path_buf());
        candidates.push(base.join("runtime").join(platform));
        candidates.push(base.join("runtime"));
    }
    candidates
}

static INIT: OnceLock<Result<(), String>> = OnceLock::new();

/// The one runtime handle for the process.
///
/// ONNX Runtime's environment, and therefore execution-provider registration, is global. A second
/// `OrtRuntime` would carry its own `registered` flag while sharing that one environment, so its
/// `register_qnn` would call `register_ep_library("QNNExecutionProvider", ..)` a second time and
/// be refused -- and the caller would read that refusal as "this machine has no NPU" and quietly
/// fall back to the CPU. One instance, one registration, one answer.
static INSTANCE: OnceLock<Arc<OrtRuntime>> = OnceLock::new();


/// One registered plugin execution provider. The handle is kept so the EP stays registered for
/// the process lifetime; dropping it would unregister the library underneath live sessions.
struct EpState {
    _lib: ExecutionProviderLibrary,
}

/// The initialized runtime.
///
/// Plugin EPs are registered **lazily** — only when a session that wants one is about to be built
/// — because a registered EP is a candidate for every session ONNX Runtime creates afterwards. A
/// globally registered QNN EP gets auto-applied to otherwise-CPU sessions and then fails on ops
/// the HTP cannot take (the encoder's dynamic `/Expand` attention mask was the real case). CPU
/// sessions are additionally pinned to the CPU device, so registration order stops mattering.
pub struct OrtRuntime {
    runtime_dir: PathBuf,
    /// Providers registered so far, keyed by accelerator. Absent = not registered.
    eps: Mutex<HashMap<Accelerator, EpState>>,
    legacy_cuda: Mutex<Option<Result<(), String>>>,
    registration_errors: Mutex<HashMap<Accelerator, String>>,
}

impl OrtRuntime {
    /// Initialize ONNX Runtime from `runtime_dir`. Does **not** register the QNN EP yet.
    ///
    /// `ort::init_from` is global and idempotent here: it runs once per process.
    pub fn init(runtime_dir: &Path) -> Result<Arc<Self>, RuntimeError> {
        // ORT and its providers may change the process working directory while loading native
        // dependencies. Keep every later provider lookup anchored to the original directory.
        let dir = runtime_dir
            .canonicalize()
            .map_err(|_| RuntimeError::NotFound(runtime_dir.display().to_string()))?;
        let lib = dir.join(onnxruntime_lib_name());
        if !lib.exists() {
            return Err(RuntimeError::NotFound(runtime_dir.display().to_string()));
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW;

            // CUDA/cuDNN are delay-loaded by ONNX Runtime. Windows otherwise searches PATH,
            // not the private runtime folder that contains their DLLs, on first convolution.
            let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe { SetDllDirectoryW(wide.as_ptr()) } == 0 {
                return Err(RuntimeError::Init(format!(
                    "could not use runtime DLL directory {}: {}",
                    dir.display(),
                    std::io::Error::last_os_error(),
                )));
            }
        }
        let res = INIT.get_or_init(|| match ort::init_from(&lib) {
            Ok(builder) => {
                builder.with_name("owlwhisp").commit();
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        });
        if let Err(e) = res {
            return Err(RuntimeError::Init(e.clone()));
        }
        if std::env::var("LW_ORT_VERBOSE").is_ok()
            && let Ok(env) = Environment::current()
        {
            env.set_log_level(ort::logging::LogLevel::Verbose);
        }

        let instance = INSTANCE.get_or_init(|| {
            Arc::new(Self {
                runtime_dir: dir.clone(),
                eps: Mutex::new(HashMap::new()),
                legacy_cuda: Mutex::new(None),
                registration_errors: Mutex::new(HashMap::new()),
            })
        });
        if instance.runtime_dir != dir {
            tracing::warn!(
                "ONNX Runtime already initialized from {}; ignoring {}",
                instance.runtime_dir.display(),
                dir.display()
            );
        }
        Ok(Arc::clone(instance))
    }

    /// Where an accelerator's provider library would live, if it can be a plugin at all.
    pub fn library_path(&self, accel: Accelerator) -> Option<PathBuf> {
        accel.library_file().map(|f| self.runtime_dir.join(f))
    }

    /// Whether the provider library for `accel` is present in the runtime directory.
    ///
    /// Presence is not usability: the library can be here and still fail to register (wrong ORT
    /// version) or register and enumerate no device (no such hardware). Use
    /// [`OrtRuntime::device_count`] for the question that actually matters.
    pub fn is_present(&self, accel: Accelerator) -> bool {
        match accel {
            Accelerator::Cpu => true,
            #[cfg(windows)]
            Accelerator::DirectMl => ort::ep::directml::api().is_some(),
            #[cfg(windows)]
            Accelerator::TensorRt => [
                "onnxruntime_providers_tensorrt.dll",
                "nvinfer_10.dll",
                "nvinfer_plugin_10.dll",
                "nvonnxparser_10.dll",
                "nvinfer_builder_resource_sm89_10.dll",
            ]
            .iter()
            .all(|file| self.runtime_dir.join(file).is_file()),
            _ => self.library_path(accel).is_some_and(|p| p.exists()),
        }
    }

    /// Register `accel`'s plugin EP if it is not registered yet. Idempotent; returns whether the
    /// provider is registered afterwards.
    ///
    /// Call this only when about to build a session that wants it: a registered EP becomes a
    /// candidate for every later session.
    pub fn register(&self, accel: Accelerator) -> bool {
        if accel == Accelerator::Cpu {
            return true; // built in, never registered
        }
        #[cfg(windows)]
        if accel == Accelerator::DirectMl {
            return ort::ep::directml::api().is_some();
        }
        if accel == Accelerator::TensorRt {
            // The official GPU package exposes TensorRT through ORT's legacy session EP.
            // Its providers_tensorrt DLL does not export the plugin CreateEpFactories symbol.
            return self.is_present(accel)
                && ort::ep::TensorRT::default().is_available().unwrap_or(false)
                && ort::ep::set_gpu_device(0).is_ok();
        }
        let mut eps = self.eps.lock();
        if eps.contains_key(&accel) {
            return true;
        }
        let (Some(name), Some(path)) = (accel.ep_name(), self.library_path(accel)) else {
            return false;
        };
        if !path.exists() {
            return false;
        }
        // Microsoft's GPU NuGet ships the legacy CUDA EP. It is compiled into its matched
        // onnxruntime.dll and cannot be registered as an external plugin library. Keep the
        // plugin path for future CUDA plugin builds, but recognize the official GPU runtime.
        if accel == Accelerator::Cuda && ort::ep::CUDA::default().is_available().unwrap_or(false) {
            let mut result = self.legacy_cuda.lock();
            let probe = result.get_or_insert_with(|| ort::ep::set_gpu_device(0).map_err(|e| e.to_string()));
            return probe.is_ok();
        }
        match Environment::current() {
            Ok(env) => match env.register_ep_library(name, &path) {
                Ok(handle) => {
                    tracing::info!("registered {name} from {}", path.display());
                    eps.insert(accel, EpState { _lib: handle });
                    true
                }
                Err(e) => {
                    tracing::warn!("{name} registration failed: {e}");
                    self.registration_errors.lock().insert(accel, e.to_string());
                    false
                }
            },
            Err(_) => false,
        }
    }

    /// Whether `accel`'s provider is registered in this process.
    pub fn registered(&self, accel: Accelerator) -> bool {
        accel == Accelerator::Cpu
            || (cfg!(windows) && accel == Accelerator::DirectMl && self.is_present(accel))
            || (accel == Accelerator::TensorRt && self.register(accel))
            || self.eps.lock().contains_key(&accel)
            || (accel == Accelerator::Cuda && self.legacy_cuda.lock().as_ref().is_some_and(Result::is_ok))
    }

    /// How many devices `accel` enumerates. Registers the provider first, lazily.
    ///
    /// This is the only honest answer to "can this machine use it": a driver package can be
    /// installed, and the library can load, while no device of the expected class appears.
    pub fn device_count(&self, accel: Accelerator) -> usize {
        if accel == Accelerator::Cpu {
            return 1;
        }
        #[cfg(windows)]
        if accel == Accelerator::DirectMl {
            return if self.register(accel) {
                Environment::current().map_or(0, |env| {
                    env.devices()
                        .filter(|d| d.ep().ok() == accel.ep_name())
                        .filter(|d| d.hardware_device().ty() == DeviceType::GPU)
                        .count()
                })
            } else {
                0
            };
        }
        if accel == Accelerator::TensorRt {
            return usize::from(self.register(accel));
        }
        if !self.register(accel) {
            return 0;
        }
        let (Some(name), Ok(env)) = (accel.ep_name(), Environment::current()) else {
            return 0;
        };
        // The legacy CUDA EP does not expose OrtEpDevice entries. Query its driver through
        // ORT instead; a successful GPU-device selection is the closest available probe.
        if accel == Accelerator::Cuda && self.eps.lock().get(&accel).is_none() {
            return usize::from(self.legacy_cuda.lock().as_ref().is_some_and(Result::is_ok));
        }
        let want = match accel.kind() {
            AcceleratorKind::Npu => Some(DeviceType::NPU),
            AcceleratorKind::Gpu => Some(DeviceType::GPU),
            AcceleratorKind::Cpu => None,
        };
        env.devices()
            .filter(|d| d.ep().map(|n| n == name).unwrap_or(false))
            .filter(|d| want.is_none_or(|t| d.hardware_device().ty() == t))
            .count()
    }

    /// Probe every accelerator this platform could have, without registering ones whose library
    /// is absent. Returns them in the automatic policy's preference order.
    pub fn probe_accelerators(&self) -> Vec<AcceleratorStatus> {
        ALL_ACCELERATORS
            .iter()
            .copied()
            .map(|accel| {
                let present = self.is_present(accel);
                // Only register providers whose library is actually here; registering is a DLL
                // load, and a failed one logs noise on every probe.
                let devices = if present { self.device_count(accel) } else { 0 };
                AcceleratorStatus {
                    accel,
                    present,
                    registered: self.registered(accel),
                    devices,
                    error: if accel == Accelerator::Cuda {
                        self.legacy_cuda
                            .lock()
                            .as_ref()
                            .and_then(|r| r.as_ref().err().cloned())
                    } else {
                        self.registration_errors.lock().get(&accel).cloned()
                    },
                }
            })
            .collect()
    }

    /// The accelerators that are actually usable here, best first.
    pub fn usable_accelerators(&self) -> Vec<Accelerator> {
        self.probe_accelerators()
            .into_iter()
            .filter(|s| s.usable())
            .map(|s| s.accel)
            .collect()
    }

    // ----- Back-compatible QNN helpers -------------------------------------------------------

    /// Whether the QNN plugin-EP library is present (i.e. NPU support *could* be available).
    pub fn qnn_available(&self) -> bool {
        self.is_present(Accelerator::QnnNpu)
    }

    /// Register the QNN plugin EP if not already registered. Idempotent.
    pub fn register_qnn(&self) -> bool {
        self.register(Accelerator::QnnNpu)
    }

    /// Convenience: locate the runtime dir and initialize.
    pub fn auto() -> Result<Arc<Self>, RuntimeError> {
        let dir =
            locate_runtime_dir().ok_or_else(|| RuntimeError::NotFound("no candidate directory".into()))?;
        Self::init(&dir)
    }

    /// The runtime directory in use.
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    /// Whether the QNN plugin EP has been registered this process.
    pub fn qnn_registered(&self) -> bool {
        self.registered(Accelerator::QnnNpu)
    }

    /// Whether an NPU device backed by the QNN EP is enumerable. Registers the QNN EP lazily.
    pub fn has_qnn_npu(&self) -> bool {
        self.device_count(Accelerator::QnnNpu) > 0
    }

    /// Number of QNN NPU devices enumerated.
    pub fn qnn_npu_count(&self) -> usize {
        self.device_count(Accelerator::QnnNpu)
    }

    /// A short description of every enumerated device (for diagnostics).
    pub fn device_summary(&self) -> Vec<String> {
        let Ok(env) = Environment::current() else {
            return Vec::new();
        };
        let mut devices: Vec<String> = env
            .devices()
            .map(|d| {
                let ep = d.ep().unwrap_or("?");
                let hw = d.hardware_device();
                let vendor = hw.vendor().unwrap_or("?");
                format!("{ep}: {vendor} {:?} (id {})", hw.ty(), hw.id())
            })
            .collect();
        if self.legacy_cuda.lock().as_ref().is_some_and(Result::is_ok) {
            devices.push("CUDAExecutionProvider: NVIDIA GPU (legacy provider, device 0)".into());
        }
        devices
    }
}

/// What one accelerator looks like on this machine, right now.
///
/// The three fields are deliberately separate because they answer different questions and are
/// routinely not the same: a vendor's driver package can be installed (`present`) and its
/// provider still refuse to load (`registered == false`), or load and enumerate nothing
/// (`devices == 0`). Only the last one means acceleration.
#[derive(Clone, Debug, serde::Serialize)]
pub struct AcceleratorStatus {
    /// Which accelerator this describes.
    pub accel: Accelerator,
    /// Its provider library is in the runtime directory.
    pub present: bool,
    /// Its provider is registered with ONNX Runtime in this process.
    pub registered: bool,
    /// How many matching devices it enumerates.
    pub devices: usize,
    /// The native loader's error when a present provider could not be used.
    pub error: Option<String>,
}

impl AcceleratorStatus {
    /// Whether this accelerator can actually be used: a device of its class enumerated.
    pub fn usable(&self) -> bool {
        self.accel == Accelerator::Cpu || self.devices > 0
    }

    /// One line explaining the verdict, suitable for diagnostics and the settings UI.
    pub fn explain(&self) -> String {
        if self.accel == Accelerator::Cpu {
            return "always available".to_string();
        }
        if !self.present {
            if self.accel == Accelerator::DirectMl {
                return "this ONNX Runtime build does not include DirectML".to_string();
            }
            return format!(
                "not installed ({} is not in the runtime directory)",
                self.accel.library_file().unwrap_or("its provider library")
            );
        }
        if !self.registered {
            return self.error.as_ref().map_or_else(
                || "provider library present but it failed to register with ONNX Runtime".to_string(),
                |e| format!("provider library present but could not load: {e}"),
            );
        }
        if self.devices == 0 {
            return "provider loaded but no matching device was found on this machine".to_string();
        }
        if self.accel == Accelerator::Cuda {
            return "CUDA provider and GPU driver loaded; a model run is needed to verify its operators"
                .to_string();
        }
        if self.accel == Accelerator::WebGpu {
            return format!(
                "{} GPU adapter(s) detected; one is selected per session, and a model run is needed to verify support",
                self.devices
            );
        }
        format!("{} device(s) available", self.devices)
    }
}

/// Compute a file's lowercase-hex SHA-256 (used by the manifest verifier).
pub(crate) fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    /// Print what every accelerator looks like on the machine running the test.
    ///
    /// Diagnostic rather than assertive: the point is to see the truth on real hardware, and the
    /// truth differs per machine. The one thing it does assert is the invariant the automatic
    /// policy depends on -- the CPU is always usable.
    #[test]
    #[ignore = "needs a staged ONNX Runtime; run explicitly"]
    fn report_accelerators_on_this_machine() {
        let dir = locate_runtime_dir().expect("a staged runtime directory");
        let rt = OrtRuntime::init(&dir).expect("init");
        println!("runtime: {}", dir.display());
        for st in rt.probe_accelerators() {
            println!(
                "  {:<28} present={:<5} registered={:<5} devices={}  -- {}",
                st.accel.label(),
                st.present,
                st.registered,
                st.devices,
                st.explain()
            );
        }
        println!("usable, best first: {:?}", rt.usable_accelerators());
        println!("all enumerated devices:");
        for d in rt.device_summary() {
            println!("  {d}");
        }
        assert!(rt.usable_accelerators().contains(&Accelerator::Cpu));
    }

    /// A second `init` must hand back the *same* runtime, or the second caller's `register_qnn`
    /// would be refused by an environment that already holds the registration -- and the NPU
    /// would silently disappear. Needs a real runtime directory, so it is ignored by default:
    /// `LW_RUNTIME_DIR=<dir> cargo test -p lw-ort -- --ignored`.
    #[test]
    #[ignore = "needs a staged ONNX Runtime; run explicitly"]
    fn init_returns_one_shared_instance_per_process() {
        let dir = locate_runtime_dir().expect("a staged runtime directory");
        let a = OrtRuntime::init(&dir).expect("init");
        let b = OrtRuntime::init(&dir).expect("init again");
        let c = OrtRuntime::auto().expect("auto");
        assert!(Arc::ptr_eq(&a, &b), "init must be a singleton");
        assert!(Arc::ptr_eq(&a, &c), "auto must return the same instance");

        if a.qnn_available() {
            // Registration must stay true across repeated calls and across handles.
            assert!(a.register_qnn(), "first registration");
            assert!(b.register_qnn(), "second handle must see it registered");
            assert!(a.qnn_registered());
        }
    }

    use super::*;

    #[test]
    fn platform_dir_matches_host() {
        let d = runtime_platform_dir();
        assert!(!d.is_empty());
        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        assert_eq!(d, "win-arm64");
    }

    #[test]
    fn candidates_prefer_explicit_then_platform_subdir() {
        let exe = Path::new("/app");
        let cwd = Path::new("/work");
        let c = runtime_candidates(
            Some(Path::new("/explicit")),
            None,
            Some(exe),
            Some(cwd),
            "win-arm64",
        );
        assert_eq!(c[0], PathBuf::from("/explicit"));
        // exe dir itself, then its runtime/<platform>, then plain runtime/
        assert_eq!(c[1], exe.to_path_buf());
        assert_eq!(c[2], exe.join("runtime").join("win-arm64"));
        assert_eq!(c[3], exe.join("runtime"));
        // then the same tree under the working directory
        assert!(c.contains(&cwd.join("runtime").join("win-arm64")));
    }

    #[test]
    fn candidates_tolerate_missing_sources() {
        assert!(runtime_candidates(None, None, None, None, "win-arm64").is_empty());
    }

    #[test]
    fn locate_finds_staged_runtime_via_env() {
        // A runtime staged as <root>/runtime/<platform>/onnxruntime.dll is found through the
        // candidate list; here we point at it directly to avoid touching the process cwd.
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("runtime").join(runtime_platform_dir());
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::File::create(staged.join(onnxruntime_lib_name())).unwrap();

        let c = runtime_candidates(None, None, Some(dir.path()), None, runtime_platform_dir());
        let found = c.into_iter().find(|d| d.join(onnxruntime_lib_name()).exists());
        assert_eq!(found.as_deref(), Some(staged.as_path()));
    }

    #[test]
    fn lib_name_is_platform_specific() {
        let n = onnxruntime_lib_name();
        assert!(n.contains("onnxruntime"));
    }

    #[test]
    fn locate_uses_env(/* isolated: sets and clears its own var */) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::File::create(dir.path().join(onnxruntime_lib_name())).unwrap();
        // SAFETY: single-threaded test; set and remove around the call.
        unsafe { std::env::set_var("LW_RUNTIME_DIR", dir.path()) };
        let found = locate_runtime_dir();
        unsafe { std::env::remove_var("LW_RUNTIME_DIR") };
        assert_eq!(found.as_deref(), Some(dir.path()));
    }

    #[test]
    fn missing_runtime_errors() {
        let dir = tempfile::tempdir().unwrap();
        match OrtRuntime::init(dir.path()) {
            Err(RuntimeError::NotFound(_)) => {}
            Ok(_) => panic!("expected NotFound"),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
}
