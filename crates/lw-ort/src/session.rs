//! Session construction: the CPU EP, and any plugin execution provider by accelerator.

use std::path::Path;

use ort::environment::Environment;
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
            .with_execution_providers([
                ort::ep::DirectML::default()
                    .with_device_filter(ort::ep::directml::DeviceFilter::Gpu)
                    .build()
                    .error_on_failure(),
            ])
            .map_err(|e| RuntimeError::Other(format!("enable DirectML: {e}")))?
            .with_optimization_level(if cfg.optimize {
                GraphOptimizationLevel::Level3
            } else {
                GraphOptimizationLevel::Disable
            })
            .map_err(|e| RuntimeError::Other(e.to_string()))?
            .commit_from_file(model_path)
            .map_err(|e| RuntimeError::Other(format!("DirectML model session {}: {e}", model_path.display())));
    }
    if accel == Accelerator::TensorRt {
        let cache_dir = model_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("tensorrt-cache");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| RuntimeError::Other(format!("create TensorRT cache: {e}")))?;
        return Session::builder()
            .map_err(|e| RuntimeError::Other(e.to_string()))?
            .with_execution_providers([
                ort::ep::TensorRT::default()
                    .with_engine_cache(true)
                    .with_engine_cache_path(cache_dir.to_string_lossy())
                    .with_timing_cache(true)
                    .build()
                    .error_on_failure(),
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
            .map_err(|e| RuntimeError::Other(format!("TensorRT model session {}: {e}", model_path.display())));
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
        return builder.commit_from_file(model_path).map_err(|e| {
            RuntimeError::Other(format!("CUDA model session {}: {e}", model_path.display()))
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default() {
        let c = CpuSessionConfig::default();
        assert!(c.optimize);
        assert_eq!(c.intra_threads, 0);
    }
}
