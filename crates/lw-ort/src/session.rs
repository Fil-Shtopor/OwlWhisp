//! Session construction: the CPU EP, and any plugin execution provider by accelerator.

use std::path::Path;

use ort::environment::Environment;
use ort::ep::ArbitrarilyConfigurableExecutionProvider;
use ort::memory::DeviceType;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

use lw_core::capabilities::Accelerator;

use crate::runtime::{OrtRuntime, RuntimeError};

/// Configuration for a CPU ONNX Runtime session.
#[derive(Clone, Copy, Debug)]
pub struct CpuSessionConfig {
    /// Intra-op thread count (0 = ORT default).
    pub intra_threads: usize,
    /// Graph optimization level.
    pub optimize: bool,
}

impl Default for CpuSessionConfig {
    fn default() -> Self {
        Self {
            intra_threads: 0,
            optimize: true,
        }
    }
}

/// Explicit shape ranges for all dynamic inputs of a TensorRT graph.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TensorRtShapeProfile {
    /// Smallest input shapes, in TensorRT's `name:1x128xT,name2:1` format.
    pub min_shapes: String,
    /// Input shapes used to choose optimized kernels.
    pub opt_shapes: String,
    /// Largest input shapes the engine must accept without rebuilding.
    pub max_shapes: String,
}

/// TensorRT options supplied by an engine that knows its input shapes.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct TensorRtSessionConfig {
    /// Permit FP16 kernels; the model's public input/output types stay unchanged.
    pub fp16: bool,
    /// A bounded input profile. `None` uses ORT's adaptive profile.
    pub profile: Option<TensorRtShapeProfile>,
}

/// Build a CPU-EP session from a model file.
///
/// The `_runtime` argument ensures ONNX Runtime has been initialized (its construction called
/// `ort::init_from`). The default execution provider is CPU, so no EP registration is needed.
pub fn build_cpu_session(
    _runtime: &OrtRuntime,
    model_path: &Path,
    cfg: CpuSessionConfig,
) -> Result<Session, RuntimeError> {
    let mut builder = Session::builder().map_err(|e| RuntimeError::Other(e.to_string()))?;
    // Pin explicitly to the CPU *device*. Appending the CPU EP alone is not enough: once a plugin
    // EP (QNN) is registered in the environment, ORT will still auto-apply it to a plain session
    // and it fails on ops the HTP can't take (the dynamic `/Expand` mask). Selecting only the CPU
    // OrtEpDevice via V2 makes the session genuinely CPU-only.
    if let Ok(env) = Environment::current() {
        let cpu_devices: Vec<_> = env
            .devices()
            .filter(|d| {
                d.ep().map(|n| n == "CPUExecutionProvider").unwrap_or(false)
                    && d.hardware_device().ty() == DeviceType::CPU
            })
            .collect();
        if !cpu_devices.is_empty() {
            builder = builder
                .with_devices(cpu_devices, None)
                .map_err(|e| RuntimeError::Other(format!("pin CPU device: {e}")))?;
        } else {
            builder = builder
                .with_execution_providers([ort::ep::CPU::default().build()])
                .map_err(|e| RuntimeError::Other(e.to_string()))?;
        }
    }
    builder = builder
        .with_optimization_level(if cfg.optimize {
            GraphOptimizationLevel::Level3
        } else {
            GraphOptimizationLevel::Disable
        })
        .map_err(|e| RuntimeError::Other(e.to_string()))?;
    if cfg.intra_threads > 0 {
        builder = builder
            .with_intra_threads(cfg.intra_threads)
            .map_err(|e| RuntimeError::Other(e.to_string()))?;
    }
    builder
        .commit_from_file(model_path)
        .map_err(|e| RuntimeError::Other(format!("commit_from_file {}: {e}", model_path.display())))
}

/// Build a session pinned to `accel`'s devices, registering its plugin EP first.
///
/// Pinning to devices rather than appending a provider matters for the same reason it matters on
/// the CPU path: once several providers are registered, ONNX Runtime will happily place a graph
/// somewhere the caller did not ask for. Selecting the devices explicitly is what makes the
/// reported backend and the executing backend the same thing.
///
/// Returns [`RuntimeError::Unsupported`] when the provider is not installed or enumerates no
/// device, so callers can fall back deliberately instead of discovering it mid-inference.
pub fn build_accel_session(
    runtime: &OrtRuntime,
    accel: Accelerator,
    model_path: &Path,
    cfg: CpuSessionConfig,
) -> Result<Session, RuntimeError> {
    build_accel_session_with_tensorrt_config(
        runtime,
        accel,
        model_path,
        cfg,
        &TensorRtSessionConfig::default(),
    )
}

/// Build an accelerator session with explicit TensorRT precision and shape options.
/// Other accelerators use the same session options as [`build_accel_session`].
pub fn build_accel_session_with_tensorrt_config(
    runtime: &OrtRuntime,
    accel: Accelerator,
    model_path: &Path,
    cfg: CpuSessionConfig,
    tensorrt: &TensorRtSessionConfig,
) -> Result<Session, RuntimeError> {
    if accel == Accelerator::Cpu {
        return build_cpu_session(runtime, model_path, cfg);
    }
    if !model_path.exists() {
        return Err(RuntimeError::NotFound(model_path.display().to_string()));
    }
    if runtime.device_count(accel) == 0 {
        return Err(RuntimeError::Unsupported(format!(
            "{}: {}",
            accel.label(),
            runtime
                .probe_accelerators()
                .into_iter()
                .find(|s| s.accel == accel)
                .map(|s| s.explain())
                .unwrap_or_else(|| "unavailable".to_string())
        )));
    }
    #[cfg(windows)]
    if accel == Accelerator::DirectMl {
        // DirectML lives in the Windows ML core. It is a legacy session EP, not an OrtEpDevice.
        // Its execution mode and memory-pattern requirements are set before registering it.
        return Session::builder()
            .map_err(|e| RuntimeError::Other(e.to_string()))?
            .with_parallel_execution(false)
            .map_err(|e| RuntimeError::Other(e.to_string()))?
            .with_memory_pattern(false)
            .map_err(|e| RuntimeError::Other(e.to_string()))?
            .with_execution_providers([ort::ep::DirectML::default()
                .with_device_filter(ort::ep::directml::DeviceFilter::Gpu)
                .build()
                .error_on_failure()])
            .map_err(|e| RuntimeError::Other(format!("enable DirectML: {e}")))?
            .with_optimization_level(if cfg.optimize {
                GraphOptimizationLevel::Level3
            } else {
                GraphOptimizationLevel::Disable
            })
            .map_err(|e| RuntimeError::Other(e.to_string()))?
            .commit_from_file(model_path)
            .map_err(|e| {
                RuntimeError::Other(format!("DirectML model session {}: {e}", model_path.display()))
            });
    }
    if accel == Accelerator::TensorRt {
        let gpu = crate::nvidia::devices()
            .map_err(RuntimeError::Other)?
            .into_iter()
            .next()
            .ok_or_else(|| RuntimeError::Unsupported("TensorRT: no NVIDIA device".into()))?;
        let identity = tensorrt_cache_identity(runtime.runtime_dir(), model_path, tensorrt, &gpu)?;
        let cache_dir = model_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("tensorrt-cache")
            .join(cache_key(&identity));
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| RuntimeError::Other(format!("create TensorRT cache: {e}")))?;
        std::fs::write(
            cache_dir.join("identity.json"),
            serde_json::to_vec_pretty(&identity).map_err(|e| RuntimeError::Other(e.to_string()))?,
        )
        .map_err(|e| RuntimeError::Other(format!("write TensorRT cache identity: {e}")))?;
        tracing::info!(cache = %cache_dir.display(), gpu = %gpu.name, "TensorRT hardware-specific cache");
        let mut provider = ort::ep::TensorRT::default()
            .with_fp16(tensorrt.fp16)
            .with_engine_cache(true)
            .with_engine_cache_path(cache_dir.to_string_lossy())
            .with_timing_cache(true)
            .with_timing_cache_path(cache_dir.to_string_lossy());
        if let Some(profile) = &tensorrt.profile {
            provider = provider
                .with_profile_min_shapes(&profile.min_shapes)
                .with_profile_opt_shapes(&profile.opt_shapes)
                .with_profile_max_shapes(&profile.max_shapes);
        }
        // V2 provider options do not inherit the deprecated ORT_TENSORRT_* environment
        // settings. Explicit diagnostic overrides let the benchmark test real profiles.
        let shape_keys = ["PROFILE_MIN_SHAPES", "PROFILE_OPT_SHAPES", "PROFILE_MAX_SHAPES"];
        let shape_count = shape_keys
            .iter()
            .filter(|key| std::env::var(format!("LW_TENSORRT_{key}")).is_ok())
            .count();
        if shape_count != 0 && shape_count != shape_keys.len() {
            return Err(RuntimeError::Other(
                "TensorRT requires all three LW_TENSORRT_PROFILE_MIN/OPT/MAX_SHAPES overrides".into(),
            ));
        }
        for key in shape_keys.into_iter().chain([
            "FP16_ENABLE",
            "MAX_WORKSPACE_SIZE",
            "BUILDER_OPTIMIZATION_LEVEL",
            "DETAILED_BUILD_LOG",
        ]) {
            if let Ok(value) = std::env::var(format!("LW_TENSORRT_{key}")) {
                tracing::info!(option = key, value, "TensorRT diagnostic override");
                provider = provider.with_arbitrary_config(format!("trt_{}", key.to_ascii_lowercase()), value);
            }
        }
        return Session::builder()
            .map_err(|e| RuntimeError::Other(e.to_string()))?
            .with_execution_providers([
                provider.build().error_on_failure(),
                ort::ep::CUDA::default().build().error_on_failure(),
            ])
            .map_err(|e| RuntimeError::Other(format!("enable TensorRT: {e}")))?
            .with_optimization_level(if cfg.optimize {
                GraphOptimizationLevel::Level3
            } else {
                GraphOptimizationLevel::Disable
            })
            .map_err(|e| RuntimeError::Other(e.to_string()))?
            .commit_from_file(model_path)
            .map_err(|e| {
                RuntimeError::Other(format!("TensorRT model session {}: {e}", model_path.display()))
            });
    }
    let name = accel
        .ep_name()
        .ok_or_else(|| RuntimeError::Unsupported(format!("{} has no provider", accel.label())))?;
    let env = Environment::current().map_err(|e| RuntimeError::Other(e.to_string()))?;
    if accel == Accelerator::Cuda && env.devices().all(|d| d.ep().ok() != Some(name)) {
        // The official GPU NuGet exposes legacy CUDA as a session EP, not an OrtEpDevice.
        // Require successful registration so a broken CUDA stack cannot silently use CPU.
        let mut builder = Session::builder()
            .map_err(|e| RuntimeError::Other(e.to_string()))?
            .with_execution_providers([ort::ep::CUDA::default().build().error_on_failure()])
            .map_err(|e| RuntimeError::Other(format!("enable CUDA: {e}")))?
            .with_optimization_level(if cfg.optimize {
                GraphOptimizationLevel::Level3
            } else {
                GraphOptimizationLevel::Disable
            })
            .map_err(|e| RuntimeError::Other(e.to_string()))?;
        return builder
            .commit_from_file(model_path)
            .map_err(|e| RuntimeError::Other(format!("CUDA model session {}: {e}", model_path.display())));
    }
    let mut devices: Vec<_> = env
        .devices()
        .filter(|d| d.ep().map(|n| n == name).unwrap_or(false))
        .collect();
    if devices.is_empty() {
        return Err(RuntimeError::Unsupported(format!(
            "{} enumerated no device",
            accel.label()
        )));
    }
    // WebGPU currently creates one device per session. A hybrid laptop can enumerate both its
    // discrete and integrated GPU; passing both makes the provider reject every model.
    if accel == Accelerator::WebGpu {
        devices.truncate(1);
    }
    let mut builder = Session::builder().map_err(|e| RuntimeError::Other(e.to_string()))?;
    builder = builder
        .with_devices(devices, None)
        .map_err(|e| RuntimeError::Other(format!("pin {} device: {e}", accel.label())))?;
    builder = builder
        .with_optimization_level(if cfg.optimize {
            GraphOptimizationLevel::Level3
        } else {
            GraphOptimizationLevel::Disable
        })
        .map_err(|e| RuntimeError::Other(e.to_string()))?;
    builder
        .commit_from_file(model_path)
        .map_err(|e| RuntimeError::Other(format!("commit_from_file {}: {e}", model_path.display())))
}

fn cache_key(identity: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(
        serde_json::to_vec(identity).expect("JSON identity"),
    ))[..24]
        .to_string()
}

fn file_identity(path: &Path) -> Result<serde_json::Value, RuntimeError> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| RuntimeError::Other(format!("cache identity {}: {e}", path.display())))?;
    Ok(
        serde_json::json!({"bytes":metadata.len(), "modified_ns":metadata.modified().ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos().to_string())}),
    )
}

fn tensorrt_cache_identity(
    runtime: &Path,
    model: &Path,
    config: &TensorRtSessionConfig,
    gpu: &crate::nvidia::NvidiaDevice,
) -> Result<serde_json::Value, RuntimeError> {
    use sha2::{Digest, Sha256};
    let mut libraries = serde_json::Map::new();
    for name in [
        crate::onnxruntime_lib_name(),
        "onnxruntime_providers_tensorrt.dll",
        "onnxruntime_providers_cuda.dll",
        "nvinfer_10.dll",
        "nvinfer_plugin_10.dll",
        "nvonnxparser_10.dll",
        "cudart64_13.dll",
        "cudnn64_9.dll",
    ] {
        let path = runtime.join(name);
        if path.is_file() {
            libraries.insert(name.into(), file_identity(&path)?);
        }
    }
    if let Some(resource) = gpu.tensorrt_resource() {
        let path = runtime.join(format!("{resource}.dll"));
        if path.is_file() {
            libraries.insert(resource, file_identity(&path)?);
        }
    }
    #[cfg(windows)]
    if let Some(system) = std::env::var_os("SystemRoot") {
        let driver = std::path::PathBuf::from(system).join("System32/nvcuda.dll");
        if driver.is_file() {
            libraries.insert("nvidia_driver".into(), file_identity(&driver)?);
        }
    }
    let mut overrides = serde_json::Map::new();
    for key in [
        "PROFILE_MIN_SHAPES",
        "PROFILE_OPT_SHAPES",
        "PROFILE_MAX_SHAPES",
        "FP16_ENABLE",
        "MAX_WORKSPACE_SIZE",
        "BUILDER_OPTIMIZATION_LEVEL",
        "DETAILED_BUILD_LOG",
    ] {
        if let Ok(value) = std::env::var(format!("LW_TENSORRT_{key}")) {
            overrides.insert(key.into(), value.into());
        }
    }
    let graph = std::fs::read(model).map_err(|e| RuntimeError::Other(e.to_string()))?;
    let weights = model.with_file_name(format!(
        "{}.data",
        model.file_name().unwrap_or_default().to_string_lossy()
    ));
    Ok(serde_json::json!({
        "schema":1, "os":std::env::consts::OS, "arch":std::env::consts::ARCH,
        "gpu":gpu, "runtime":libraries, "model_sha256":hex::encode(Sha256::digest(graph)),
        "weights":if weights.is_file() { Some(file_identity(&weights)?) } else { None },
        "config":config, "overrides":overrides,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default() {
        let c = CpuSessionConfig::default();
        assert!(c.optimize);
        assert_eq!(c.intra_threads, 0);
    }

    #[test]
    fn tensorrt_cache_changes_when_gpu_runtime_model_or_precision_changes() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("encoder.onnx");
        let weights = dir.path().join("encoder.onnx.data");
        let library = dir.path().join(crate::onnxruntime_lib_name());
        std::fs::write(&model, b"graph one").unwrap();
        std::fs::write(&weights, b"weights").unwrap();
        std::fs::write(&library, b"runtime").unwrap();
        let mut gpu = crate::nvidia::NvidiaDevice {
            ordinal: 0,
            name: "same model name".into(),
            uuid: "first".into(),
            major: 8,
            minor: 9,
            driver_cuda_version: 13000,
        };
        let mut config = TensorRtSessionConfig::default();
        let key = |g: &crate::nvidia::NvidiaDevice, c: &TensorRtSessionConfig| {
            cache_key(&tensorrt_cache_identity(dir.path(), &model, c, g).unwrap())
        };
        let first = key(&gpu, &config);
        assert_eq!(first, key(&gpu, &config));
        gpu.uuid = "different physical GPU".into();
        assert_ne!(first, key(&gpu, &config));
        gpu.uuid = "first".into();
        gpu.driver_cuda_version = 13010;
        assert_ne!(first, key(&gpu, &config));
        gpu.driver_cuda_version = 13000;
        config.fp16 = true;
        assert_ne!(first, key(&gpu, &config));
        config.fp16 = false;
        std::fs::write(&model, b"graph two").unwrap();
        assert_ne!(first, key(&gpu, &config));
        std::fs::write(&model, b"graph one").unwrap();
        std::fs::write(&weights, b"different weights length").unwrap();
        assert_ne!(first, key(&gpu, &config));
        let previous = key(&gpu, &config);
        std::fs::write(&library, b"new runtime version").unwrap();
        assert_ne!(previous, key(&gpu, &config));
    }
}
