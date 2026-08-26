//! # lw-vad-silero
//!
//! Silero VAD (ONNX) implementation of [`lw_core::vad::Vad`], running on the ORT CPU EP via
//! [`lw_ort`]. Silero v5/v6 takes one 512-sample frame at 16 kHz plus a carried LSTM state
//! `[2,1,128]` and a sample-rate scalar, and returns a speech probability in `[0,1]`.
//!
//! The endpoint state machine that turns per-frame probabilities into speech segments lives in
//! `lw_core::vad` and is model-agnostic; this crate only produces the probabilities.
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use std::path::Path;

use lw_core::vad::{Vad, VAD_FRAME_SIZE};
use lw_ort::ort::session::Session;
use lw_ort::ort::value::Tensor;
use lw_ort::{build_cpu_session, CpuSessionConfig, OrtRuntime};

/// Errors from the Silero VAD.
#[derive(Debug, thiserror::Error)]
pub enum SileroError {
    /// The model file was not found.
    #[error("silero model not found: {0}")]
    NotFound(String),
    /// ONNX Runtime error.
    #[error("onnxruntime: {0}")]
    Ort(String),
}

impl From<SileroError> for lw_core::Error {
    fn from(e: SileroError) -> Self {
        lw_core::Error::Other(e.to_string())
    }
}

const STATE_LEN: usize = 2 * 1 * 128;

/// Silero VAD detector.
pub struct SileroVad {
    session: Session,
    state: Vec<f32>,
    sample_rate: i64,
}

impl SileroVad {
    /// Load `silero_vad.onnx` from `path` on the CPU EP.
    pub fn load(runtime: &OrtRuntime, path: &Path) -> Result<Self, SileroError> {
        if !path.exists() {
            return Err(SileroError::NotFound(path.display().to_string()));
        }
        let session = build_cpu_session(runtime, path, CpuSessionConfig { intra_threads: 1, optimize: true })
            .map_err(|e| SileroError::Ort(e.to_string()))?;
        Ok(Self { session, state: vec![0.0; STATE_LEN], sample_rate: 16_000 })
    }
}

impl Vad for SileroVad {
    fn process_frame(&mut self, frame: &[f32]) -> lw_core::Result<f32> {
        // Feed exactly one 512-sample frame; pad if a short tail frame is given.
        let mut buf = vec![0.0f32; VAD_FRAME_SIZE];
        let n = frame.len().min(VAD_FRAME_SIZE);
        buf[..n].copy_from_slice(&frame[..n]);

        let input = Tensor::from_array((vec![1i64, VAD_FRAME_SIZE as i64], buf))
            .map_err(|e| lw_core::Error::Other(e.to_string()))?;
        let state = Tensor::from_array((vec![2i64, 1i64, 128i64], self.state.clone()))
            .map_err(|e| lw_core::Error::Other(e.to_string()))?;
        let sr = Tensor::from_array((Vec::<i64>::new(), vec![self.sample_rate]))
            .map_err(|e| lw_core::Error::Other(e.to_string()))?;

        let outputs = self
            .session
            .run(lw_ort::ort::inputs!["input" => input, "state" => state, "sr" => sr])
            .map_err(|e| lw_core::Error::Other(e.to_string()))?;

        // Carry the new state.
        if let Some(new_state) = outputs.get("stateN").and_then(|v| v.try_extract_array::<f32>().ok()) {
            let v: Vec<f32> = new_state.iter().copied().collect();
            if v.len() == STATE_LEN {
                self.state = v;
            }
        }
        let prob = outputs
            .get("output")
            .and_then(|v| v.try_extract_array::<f32>().ok())
            .and_then(|a| a.iter().next().copied())
            .unwrap_or(0.0);
        Ok(prob.clamp(0.0, 1.0))
    }

    fn reset(&mut self) {
        self.state = vec![0.0; STATE_LEN];
    }
}

#[cfg(test)]
mod tests {
    // Model-loading tests require the Silero model and the ORT runtime; they run as ignored
    // integration tests in `tests/`. Here we only test the state length constant.
    #[test]
    fn state_len() {
        assert_eq!(super::STATE_LEN, 256);
    }
}
