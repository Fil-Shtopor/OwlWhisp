//! Encoder backends: the only stage that moves between CPU and NPU.
//!
//! - [`CpuEncoder`]: dynamic-shape ONNX encoder on the ORT CPU EP (int8 or fp32). Universal.
//! - [`QnnHtpEncoder`]: static-shape ONNX encoder on the Qualcomm Hexagon HTP via ORT's QNN EP,
//!   fp16, with an on-device-prepared EPContext cache. Audio is zero-padded to the static window
//!   and the valid frame count is clipped so padding produces no trailing tokens.
//!
//! Both take mel features `[128, T]` (row-major `mel*T + frame`) and return encoder output
//! `[D, T']` (row-major `d*T' + t`) plus the valid `T'`.

use std::path::Path;

use lw_ort::ort::session::Session;
use lw_ort::ort::value::Tensor;
use lw_ort::{CpuSessionConfig, OrtRuntime, QnnSessionConfig, build_cpu_session, build_qnn_session};

use crate::mel::N_MELS;
use crate::{Error, Result};

/// Encoder output feature dimension (Parakeet v3 FastConformer).
pub const ENC_DIM: usize = 1024;
/// Encoder subsampling factor (8×).
pub const SUBSAMPLING: usize = 8;

/// A pluggable encoder.
pub trait EncoderBackend: Send {
    /// Run the encoder on mel features `[N_MELS, n_frames]`.
    /// Returns `(encoder_out[ENC_DIM * t_out], t_out)`.
    fn run(&mut self, feats: &[f32], n_frames: usize) -> Result<(Vec<f32>, usize)>;
    /// A short device/provider label for diagnostics.
    fn label(&self) -> String;
}

/// Read the encoder output tensor `[1, D, T']` into a flat `[D*T']` plus `T'` and valid length.
fn read_encoder_output(
    outputs: &lw_ort::ort::session::SessionOutputs<'_>,
    valid_frames_hint: Option<usize>,
) -> Result<(Vec<f32>, usize)> {
    // Output tensor name is "outputs" for istupakov/sherpa exports; "output_0" for AI-Hub wrappers.
    let out = outputs
        .get("outputs")
        .or_else(|| outputs.get("output_0"))
        .ok_or_else(|| Error::Decode("encoder output tensor not found".into()))?;
    let arr = out.try_extract_array::<f32>()?;
    let shape = arr.shape().to_vec();
    if shape.len() != 3 || shape[1] != ENC_DIM {
        return Err(Error::Decode(format!(
            "unexpected encoder output shape {shape:?}"
        )));
    }
    let t_out = shape[2];
    let flat: Vec<f32> = arr.iter().copied().collect();
    // Determine valid length from the length output if present, else the hint, else all frames.
    let len_out = outputs.get("encoded_lengths").or_else(|| outputs.get("output_1"));
    let valid = if let Some(l) = len_out {
        l.try_extract_array::<i64>()
            .ok()
            .and_then(|a| a.iter().next().copied())
            .map(|v| v as usize)
            .or_else(|| {
                l.try_extract_array::<i32>()
                    .ok()
                    .and_then(|a| a.iter().next().copied())
                    .map(|v| v as usize)
            })
            .unwrap_or(t_out)
    } else {
        valid_frames_hint.unwrap_or(t_out)
    };
    Ok((flat, valid.min(t_out)))
}

/// CPU encoder (dynamic shapes).
pub struct CpuEncoder {
    session: Session,
    label: String,
}

impl CpuEncoder {
    /// Load a dynamic-shape encoder ONNX (int8 or fp32) on the CPU EP.
    pub fn load(runtime: &OrtRuntime, path: &Path, threads: usize) -> Result<Self> {
        if !path.exists() {
            return Err(Error::MissingFile(path.display().to_string()));
        }
        let cfg = CpuSessionConfig {
            intra_threads: threads,
            optimize: true,
        };
        let session = build_cpu_session(runtime, path, cfg).map_err(|e| Error::Ort(e.to_string()))?;
        Ok(Self {
            session,
            label: format!(
                "CPU ({} threads)",
                if threads == 0 { num_cpus_hint() } else { threads }
            ),
        })
    }
}

impl EncoderBackend for CpuEncoder {
    fn run(&mut self, feats: &[f32], n_frames: usize) -> Result<(Vec<f32>, usize)> {
        if n_frames == 0 {
            return Ok((Vec::new(), 0));
        }
        tracing::debug!(
            "cpu encoder: feats.len={} n_frames={} expected={}",
            feats.len(),
            n_frames,
            N_MELS * n_frames
        );
        if feats.len() != N_MELS * n_frames {
            return Err(Error::Feature(format!(
                "feature length {} != {} (128*{})",
                feats.len(),
                N_MELS * n_frames,
                n_frames
            )));
        }
        let audio_signal = Tensor::from_array((vec![1i64, N_MELS as i64, n_frames as i64], feats.to_vec()))?;
        let length = Tensor::from_array((vec![1i64], vec![n_frames as i64]))?;
        let outputs = self
            .session
            .run(lw_ort::ort::inputs!["audio_signal" => audio_signal, "length" => length])?;
        read_encoder_output(&outputs, Some(n_frames / SUBSAMPLING))
    }
    fn label(&self) -> String {
        self.label.clone()
    }
}

/// An encoder that runs a **static-shape** graph over a fixed mel window.
///
/// Both accelerated paths use it, for different reasons that happen to want the same shape:
/// the Hexagon HTP requires static shapes to compile a context at all, and a GPU avoids a
/// re-plan per input length. The only difference between them is how the session was built, so
/// the windowing, padding and trimming live here once.
pub struct StaticWindowEncoder {
    session: Session,
    window_frames: usize,
    label: String,
}

impl StaticWindowEncoder {
    /// Build/load an HTP encoder for a static `window_frames`-length mel window.
    ///
    /// On first use this prepares and caches the QNN context (minutes); afterwards it loads the
    /// cached context (~seconds). Errors bubble up so the engine can fall back.
    pub fn qnn(
        runtime: &OrtRuntime,
        path: &Path,
        window_frames: usize,
        cfg: QnnSessionConfig,
        device_label: impl Into<String>,
    ) -> Result<Self> {
        if !path.exists() {
            return Err(Error::MissingFile(path.display().to_string()));
        }
        let session = build_qnn_session(runtime, path, &cfg).map_err(|e| Error::Ort(e.to_string()))?;
        Ok(Self {
            session,
            window_frames,
            label: device_label.into(),
        })
    }

    /// Load the same static graph onto any other accelerator's execution provider.
    ///
    /// Unlike the QNN path this compiles nothing ahead of time and caches nothing: a GPU provider
    /// consumes the ordinary graph, which is exactly why one artifact serves every GPU vendor.
    pub fn on_accelerator(
        runtime: &OrtRuntime,
        accel: lw_core::capabilities::Accelerator,
        path: &Path,
        window_frames: usize,
        threads: usize,
        device_label: impl Into<String>,
    ) -> Result<Self> {
        if !path.exists() {
            return Err(Error::MissingFile(path.display().to_string()));
        }
        let cfg = CpuSessionConfig {
            intra_threads: threads,
            optimize: true,
        };
        let session = lw_ort::build_accel_session(runtime, accel, path, cfg).map_err(|e| match e {
            // "unavailable" already reads as a plain sentence naming the provider and the
            // reason; tagging it as an onnxruntime error only adds noise for the reader.
            lw_ort::RuntimeError::Unsupported(msg) => Error::Other(msg),
            other => Error::Ort(other.to_string()),
        })?;
        Ok(Self {
            session,
            window_frames,
            label: device_label.into(),
        })
    }

    /// The static window length in mel frames.
    pub fn window_frames(&self) -> usize {
        self.window_frames
    }
}

impl EncoderBackend for StaticWindowEncoder {
    fn run(&mut self, feats: &[f32], n_frames: usize) -> Result<(Vec<f32>, usize)> {
        if n_frames == 0 {
            return Ok((Vec::new(), 0));
        }
        let t = self.window_frames;
        // Zero-pad (or trim) the [128, n_frames] features into a [128, t] window.
        let mut padded = vec![0.0f32; N_MELS * t];
        let copy = n_frames.min(t);
        for m in 0..N_MELS {
            for f in 0..copy {
                padded[m * t + f] = feats[m * n_frames + f];
            }
        }
        let audio_signal = Tensor::from_array((vec![1i64, N_MELS as i64, t as i64], padded))?;
        let length = Tensor::from_array((vec![1i64], vec![copy as i64]))?;
        let outputs = self
            .session
            .run(lw_ort::ort::inputs!["audio_signal" => audio_signal, "length" => length])?;
        // Clip valid encoder frames to those derived from real (unpadded) mel frames.
        let valid_hint = copy.div_ceil(SUBSAMPLING);
        read_encoder_output(&outputs, Some(valid_hint))
    }
    fn label(&self) -> String {
        self.label.clone()
    }
}

fn num_cpus_hint() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

#[cfg(test)]
mod tests {
    #[test]
    fn constants() {
        assert_eq!(super::ENC_DIM, 1024);
        assert_eq!(super::SUBSAMPLING, 8);
    }
}
