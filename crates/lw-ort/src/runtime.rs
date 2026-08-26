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

/// Search for a directory containing `onnxruntime`. Order:
/// 1. `LW_RUNTIME_DIR` env var,
/// 2. `ORT_DYLIB_PATH` env var (its parent directory),
/// 3. the executable's directory and a `runtime/` subdir,
/// 4. the current working directory.
pub fn locate_runtime_dir() -> Option<PathBuf> {
    let lib = onnxruntime_lib_name();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("LW_RUNTIME_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    if let Ok(p) = std::env::var("ORT_DYLIB_PATH")
        && let Some(parent) = Path::new(&p).parent()
    {
        candidates.push(parent.to_path_buf());
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.to_path_buf());
        candidates.push(dir.join("runtime"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    candidates.into_iter().find(|d| d.join(lib).exists())
}

static INIT: OnceLock<Result<(), String>> = OnceLock::new();

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

        Ok(Arc::new(Self {
            runtime_dir: dir,
            qnn_dll: runtime_dir.join(qnn_provider_lib_name()),
            qnn: Mutex::new(QnnState {
                registered: false,
                _lib: None,
            }),
        }))
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
    use super::*;

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
