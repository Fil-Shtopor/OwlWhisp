//! QNN / Hexagon HTP session construction with on-device EPContext caching.
//!
//! The flow mirrors the verified experiment (`docs/x2-npu.md`): register the QNN EP (done once by
//! [`OrtRuntime`]), select the NPU device, and build a session with `backend_type=htp`,
//! `enable_htp_fp16_precision=1`, `htp_performance_mode=burst`. On first use we ask ORT to write an
//! EPContext cache (`ep.context_enable=1`); on later uses we load the cached `_ctx.onnx` directly,
//! skipping the multi-minute graph-prepare.

use std::path::{Path, PathBuf};

use ort::environment::Environment;
use ort::memory::DeviceType;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

use crate::runtime::{OrtRuntime, RuntimeError};

/// HTP performance profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HtpPerformanceMode {
    /// Lowest latency (recommended for interactive dictation).
    #[default]
    Burst,
    /// Balanced.
    HighPerformance,
    /// Power-saving.
    PowerSaver,
}

impl HtpPerformanceMode {
    fn as_str(&self) -> &'static str {
        match self {
            HtpPerformanceMode::Burst => "burst",
            HtpPerformanceMode::HighPerformance => "high_performance",
            HtpPerformanceMode::PowerSaver => "power_saver",
        }
    }
}

/// Configuration for a QNN/HTP session.
#[derive(Clone, Debug)]
pub struct QnnSessionConfig {
    /// Use fp16 precision on HTP (no quantization required).
    pub fp16: bool,
    /// HTP performance mode.
    pub performance: HtpPerformanceMode,
    /// Graph-finalization optimization mode (0..=3; 3 = most aggressive).
    pub finalization_opt: u8,
    /// Directory where the EPContext cache (`*_ctx.onnx` + `*_qnn.bin`) is stored.
    pub cache_dir: PathBuf,
    /// A stable cache key (e.g. model hash + qairt + arch) used to name the cache file.
    pub cache_key: String,
    /// `htp_arch` override (e.g. `81`); `None` lets the driver decide.
    pub htp_arch: Option<u32>,
    /// `soc_model` override (e.g. `88`); `None` lets the driver decide.
    pub soc_model: Option<u32>,
    /// Clamp fp16 overflow (recommended on V79+/V81 with QAIRT ≥ 2.49).
    pub fp16_clamp_overflow: bool,
}

impl QnnSessionConfig {
    /// A sensible default for a given cache directory and key.
    pub fn new(cache_dir: impl Into<PathBuf>, cache_key: impl Into<String>) -> Self {
        Self {
            fp16: true,
            performance: HtpPerformanceMode::Burst,
            finalization_opt: 3,
            cache_dir: cache_dir.into(),
            cache_key: cache_key.into(),
            htp_arch: None,
            soc_model: None,
            fp16_clamp_overflow: true,
        }
    }

    /// Path of the EPContext wrapper ONNX for this key.
    pub fn context_path(&self) -> PathBuf {
        self.cache_dir.join(format!("{}_ctx.onnx", sanitize(&self.cache_key)))
    }

    /// Provider options (prefixed with the EP name, as `with_devices` requires).
    fn provider_options(&self) -> Vec<(String, String)> {
        let ep = "QNNExecutionProvider";
        let mut opts = vec![
            (format!("{ep}.backend_type"), "htp".to_string()),
            (format!("{ep}.htp_performance_mode"), self.performance.as_str().to_string()),
            (
                format!("{ep}.htp_graph_finalization_optimization_mode"),
                self.finalization_opt.to_string(),
            ),
        ];
        if self.fp16 {
            opts.push((format!("{ep}.enable_htp_fp16_precision"), "1".into()));
        }
        if self.fp16_clamp_overflow {
            opts.push((format!("{ep}.enable_htp_fp16_clamp_overflow"), "1".into()));
        }
        if let Some(a) = self.htp_arch {
            opts.push((format!("{ep}.htp_arch"), a.to_string()));
        }
        if let Some(s) = self.soc_model {
            opts.push((format!("{ep}.soc_model"), s.to_string()));
        }
        opts
    }
}

fn sanitize(key: &str) -> String {
    key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// Build a QNN/HTP session for `model_path`, preparing and caching the context on first use and
/// loading the cache thereafter.
///
/// Returns an error if no QNN NPU device is available (the caller falls back to CPU).
pub fn build_qnn_session(
    runtime: &OrtRuntime,
    model_path: &Path,
    cfg: &QnnSessionConfig,
) -> Result<Session, RuntimeError> {
    if !runtime.register_qnn() {
        return Err(RuntimeError::Qnn("QNN EP not available/registrable".into()));
    }
    std::fs::create_dir_all(&cfg.cache_dir)
        .map_err(|e| RuntimeError::Qnn(format!("cache dir: {e}")))?;

    let ctx_path = cfg.context_path();
    let cache_hit = ctx_path.exists();
    let source = if cache_hit { &ctx_path } else { model_path };

    let env = Environment::current().map_err(|e| RuntimeError::Qnn(e.to_string()))?;
    let npu_devices: Vec<_> = env
        .devices()
        .filter(|d| {
            d.ep().map(|n| n == "QNNExecutionProvider").unwrap_or(false)
                && d.hardware_device().ty() == DeviceType::NPU
        })
        .collect();
    if npu_devices.is_empty() {
        return Err(RuntimeError::Qnn("no QNN NPU device enumerated".into()));
    }

    let opts = cfg.provider_options();
    let mut builder = Session::builder().map_err(|e| RuntimeError::Qnn(e.to_string()))?;
    // Basic optimization only; heavy CPU-side fusions can confuse the HTP partitioner.
    builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level1)
        .map_err(|e| RuntimeError::Qnn(e.to_string()))?;

    if !cache_hit {
        // Ask ORT to emit an EPContext cache next to `ctx_path`, non-embedded (separate .bin).
        builder = builder
            .with_config_entry("ep.context_enable", "1")
            .and_then(|b| b.with_config_entry("ep.context_file_path", &ctx_path.to_string_lossy()))
            .and_then(|b| b.with_config_entry("ep.context_embed_mode", "0"))
            .map_err(|e| RuntimeError::Qnn(e.to_string()))?;
    }

    builder = builder
        .with_devices(npu_devices, Some(&opts))
        .map_err(|e| RuntimeError::Qnn(format!("with_devices: {e}")))?;

    let session = builder
        .commit_from_file(source)
        .map_err(|e| RuntimeError::Qnn(format!("commit_from_file {}: {e}", source.display())))?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_path_is_sanitized() {
        let cfg = QnnSessionConfig::new("/cache", "parakeet/v3:81");
        assert!(cfg.context_path().to_string_lossy().ends_with("parakeet_v3_81_ctx.onnx"));
    }

    #[test]
    fn provider_options_include_htp() {
        let cfg = QnnSessionConfig::new("/cache", "k");
        let opts = cfg.provider_options();
        assert!(opts.iter().any(|(k, v)| k.ends_with(".backend_type") && v == "htp"));
        assert!(opts.iter().any(|(k, v)| k.ends_with(".enable_htp_fp16_precision") && v == "1"));
        assert!(opts.iter().any(|(k, v)| k.ends_with(".htp_performance_mode") && v == "burst"));
    }

    #[test]
    fn arch_and_soc_optional() {
        let mut cfg = QnnSessionConfig::new("/cache", "k");
        cfg.htp_arch = Some(81);
        cfg.soc_model = Some(88);
        let opts = cfg.provider_options();
        assert!(opts.iter().any(|(k, v)| k.ends_with(".htp_arch") && v == "81"));
        assert!(opts.iter().any(|(k, v)| k.ends_with(".soc_model") && v == "88"));
    }
}
