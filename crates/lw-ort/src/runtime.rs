//! Runtime discovery + one-time ONNX Runtime initialization + QNN EP registration.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use ort::environment::Environment;
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

/// The QNN plugin-EP library file name (Windows only in practice).
pub fn qnn_provider_lib_name() -> &'static str {
    "onnxruntime_providers_qnn.dll"
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
/// 3. the executable's directory, then `runtime/<platform>/` and `runtime/` beside it,
/// 4. the current working directory (and the same two subdirs).
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
    for base in [exe_dir, cwd].into_iter().flatten() {
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

struct QnnState {
    registered: bool,
    /// Kept alive so the EP stays registered for the process lifetime.
    _lib: Option<ExecutionProviderLibrary>,
}

/// The initialized runtime. The QNN plugin EP is registered **lazily** (only when an NPU session is
/// actually requested), because registering it globally causes ONNX Runtime to auto-apply it to
/// otherwise-CPU sessions — which then fail on ops the HTP can't take (e.g. the dynamic `/Expand`
/// attention mask). CPU sessions are built before any QNN registration and pinned to the CPU EP.
pub struct OrtRuntime {
    runtime_dir: PathBuf,
    qnn_dll: PathBuf,
    qnn: Mutex<QnnState>,
}

impl OrtRuntime {
    /// Initialize ONNX Runtime from `runtime_dir`. Does **not** register the QNN EP yet.
    ///
    /// `ort::init_from` is global and idempotent here: it runs once per process.
    pub fn init(runtime_dir: &Path) -> Result<Arc<Self>, RuntimeError> {
        let lib = runtime_dir.join(onnxruntime_lib_name());
        if !lib.exists() {
            return Err(RuntimeError::NotFound(runtime_dir.display().to_string()));
        }
        let dir = runtime_dir.to_path_buf();
        let res = INIT.get_or_init(|| match ort::init_from(&lib) {
            Ok(builder) => {
                builder.with_name("localwisper").commit();
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
                qnn_dll: dir.join(qnn_provider_lib_name()),
                qnn: Mutex::new(QnnState {
                    registered: false,
                    _lib: None,
                }),
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

    /// Whether the QNN plugin-EP DLL is present (i.e. NPU support *could* be available).
    pub fn qnn_available(&self) -> bool {
        self.qnn_dll.exists()
    }

    /// Register the QNN plugin EP if not already registered. Idempotent.
    ///
    /// Call this only when about to build an NPU session — after all CPU sessions are built.
    pub fn register_qnn(&self) -> bool {
        let mut state = self.qnn.lock();
        if state.registered {
            return true;
        }
        if !self.qnn_dll.exists() {
            return false;
        }
        match Environment::current() {
            Ok(env) => match env.register_ep_library("QNNExecutionProvider", &self.qnn_dll) {
                Ok(handle) => {
                    state._lib = Some(handle);
                    state.registered = true;
                    true
                }
                Err(e) => {
                    tracing::warn!("QNN EP registration failed: {e}");
                    false
                }
            },
            Err(_) => false,
        }
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
        self.qnn.lock().registered
    }

    /// Whether an NPU device backed by the QNN EP is enumerable. Registers the QNN EP lazily.
    pub fn has_qnn_npu(&self) -> bool {
        if !self.register_qnn() {
            return false;
        }
        self.qnn_npu_count() > 0
    }

    /// Number of QNN NPU devices enumerated.
    pub fn qnn_npu_count(&self) -> usize {
        let Ok(env) = Environment::current() else {
            return 0;
        };
        env.devices()
            .filter(|d| {
                d.ep().map(|n| n == "QNNExecutionProvider").unwrap_or(false)
                    && d.hardware_device().ty() == DeviceType::NPU
            })
            .count()
    }

    /// A short description of every enumerated device (for diagnostics).
    pub fn device_summary(&self) -> Vec<String> {
        let Ok(env) = Environment::current() else {
            return Vec::new();
        };
        env.devices()
            .map(|d| {
                let ep = d.ep().unwrap_or("?");
                let hw = d.hardware_device();
                let vendor = hw.vendor().unwrap_or("?");
                format!("{ep}: {vendor} {:?} (id {})", hw.ty(), hw.id())
            })
            .collect()
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
