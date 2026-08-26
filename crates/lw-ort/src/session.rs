//! CPU session construction helpers.

use std::path::Path;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

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
        Self { intra_threads: 0, optimize: true }
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
