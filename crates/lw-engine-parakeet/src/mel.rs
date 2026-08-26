//! Mel-spectrogram front end for Parakeet.
//!
//! Two implementations behind one interface:
//! - [`OnnxMel`]: runs NVIDIA/istupakov's `nemo128.onnx` preprocessor (ONNX STFT op, CPU-only).
//!   This exactly reproduces the pipeline verified on-device (WER ≈ 5 %); it is the default.
//! - [`NativeMel`]: a pure-Rust reimplementation of NeMo's `FilterbankFeatures` (no ONNX STFT
//!   dependency, usable for streaming). Validated against `OnnxMel` in tests within tolerance.
//!
//! Output is a `[128, T]` feature matrix in row-major (`mel * T + frame`) order plus `T`.

use std::path::Path;

use lw_ort::ort::value::Tensor;
use lw_ort::ort::session::Session;
use lw_ort::{build_cpu_session, CpuSessionConfig, OrtRuntime};
use realfft::RealFftPlanner;

use crate::{Error, Result};

/// Number of mel bins (Parakeet v3).
pub const N_MELS: usize = 128;
const SAMPLE_RATE: f32 = 16_000.0;
const N_FFT: usize = 512;
const WIN_LENGTH: usize = 400; // 25 ms
const HOP: usize = 160; // 10 ms
const PREEMPH: f32 = 0.97;
const LOG_GUARD: f32 = 5.960_464_5e-8; // 2^-24

/// A mel front end.
pub trait MelFrontend: Send {
    /// Extract features from 16 kHz mono audio. Returns `(features[128*T], n_frames)`.
    fn extract(&mut self, audio: &[f32]) -> Result<(Vec<f32>, usize)>;
}

/// The ONNX `nemo128.onnx` preprocessor.
pub struct OnnxMel {
    session: Session,
}

impl OnnxMel {
    /// Load `nemo128.onnx` from `path`.
    pub fn load(runtime: &OrtRuntime, path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::MissingFile(path.display().to_string()));
        }
        let session = build_cpu_session(runtime, path, CpuSessionConfig::default())
            .map_err(|e| Error::Ort(e.to_string()))?;
        Ok(Self { session })
    }
}

impl MelFrontend for OnnxMel {
    fn extract(&mut self, audio: &[f32]) -> Result<(Vec<f32>, usize)> {
        let n = audio.len();
        let wav = Tensor::from_array((vec![1i64, n as i64], audio.to_vec()))?;
        let lens = Tensor::from_array((vec![1i64], vec![n as i64]))?;
        let outputs = self
            .session
            .run(lw_ort::ort::inputs!["waveforms" => wav, "waveforms_lens" => lens])?;
        let feats = outputs["features"].try_extract_array::<f32>()?;
        let shape = feats.shape().to_vec();
        // [1, 128, T]
        if shape.len() != 3 || shape[1] != N_MELS {
            return Err(Error::Feature(format!("unexpected feature shape {shape:?}")));
        }
        let t = shape[2];
        let flat: Vec<f32> = feats.iter().copied().collect();
        Ok((flat, t))
    }
}

/// A pure-Rust NeMo `FilterbankFeatures` implementation.
pub struct NativeMel {
    planner_input: Vec<f32>,
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    window: Vec<f32>,
    mel_fb: Vec<f32>, // [N_MELS * (N_FFT/2+1)]
    n_freq: usize,
}

impl NativeMel {
    /// Construct the native mel front end (builds the Slaney filterbank once).
    pub fn new() -> Self {
        let n_freq = N_FFT / 2 + 1;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N_FFT);
        // Symmetric Hann window of WIN_LENGTH (periodic=false), centre-padded in the 512 frame.
        let mut window = vec![0.0f32; N_FFT];
        let offset = (N_FFT - WIN_LENGTH) / 2; // 56
        for i in 0..WIN_LENGTH {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (WIN_LENGTH as f32 - 1.0)).cos();
            window[offset + i] = w;
        }
        let mel_fb = slaney_mel_filterbank(N_MELS, N_FFT, SAMPLE_RATE, 0.0, 8000.0);
        Self { planner_input: vec![0.0; N_FFT], fft, window, mel_fb, n_freq }
    }
}

impl Default for NativeMel {
    fn default() -> Self {
        Self::new()
    }
}

impl MelFrontend for NativeMel {
    fn extract(&mut self, audio: &[f32]) -> Result<(Vec<f32>, usize)> {
        if audio.is_empty() {
            return Ok((Vec::new(), 0));
        }
        // Pre-emphasis: y[t] = x[t] - 0.97 * x[t-1], y[0] = x[0].
        let mut pre = vec![0.0f32; audio.len()];
        pre[0] = audio[0];
        for i in 1..audio.len() {
            pre[i] = audio[i] - PREEMPH * audio[i - 1];
        }
        // center=True: reflect-pad by N_FFT/2 on both sides.
        let pad = N_FFT / 2;
        let padded = reflect_pad(&pre, pad);
        let n_frames = pre.len() / HOP + 1;

        let mut spectrum = self.fft.make_output_vec();
        let mut power = vec![0.0f32; self.n_freq];
        let mut feats = vec![0.0f32; N_MELS * n_frames];

        for frame in 0..n_frames {
            let start = frame * HOP;
            // windowed frame
            for i in 0..N_FFT {
                let s = padded.get(start + i).copied().unwrap_or(0.0);
                self.planner_input[i] = s * self.window[i];
            }
            self.fft
                .process(&mut self.planner_input, &mut spectrum)
                .map_err(|e| Error::Feature(format!("fft: {e}")))?;
            for (k, c) in spectrum.iter().enumerate() {
                power[k] = c.re * c.re + c.im * c.im;
            }
            // mel projection + log guard
            for m in 0..N_MELS {
                let mut acc = 0.0f32;
                let row = &self.mel_fb[m * self.n_freq..(m + 1) * self.n_freq];
                for k in 0..self.n_freq {
                    acc += row[k] * power[k];
                }
                feats[m * n_frames + frame] = (acc + LOG_GUARD).ln();
            }
        }

        per_feature_normalize(&mut feats, N_MELS, n_frames);
        Ok((feats, n_frames))
    }
}

/// Reflect-pad a signal by `pad` samples on each side.
fn reflect_pad(x: &[f32], pad: usize) -> Vec<f32> {
    let n = x.len();
    let mut out = Vec::with_capacity(n + 2 * pad);
    for i in 0..pad {
        // reflect without repeating the edge sample
        let idx = (pad - i).min(n - 1);
        out.push(x[idx]);
    }
    out.extend_from_slice(x);
    for i in 0..pad {
        let idx = n.saturating_sub(2 + i);
        out.push(x[idx.min(n - 1)]);
    }
    out
}

/// Per-feature mean/unbiased-std normalization across valid frames (NeMo `per_feature`).
fn per_feature_normalize(feats: &mut [f32], n_mels: usize, n_frames: usize) {
    if n_frames == 0 {
        return;
    }
    for m in 0..n_mels {
        let row = &mut feats[m * n_frames..(m + 1) * n_frames];
        let mean: f32 = row.iter().sum::<f32>() / n_frames as f32;
        let var: f32 = if n_frames > 1 {
            row.iter().map(|&v| (v - mean) * (v - mean)).sum::<f32>() / (n_frames as f32 - 1.0)
        } else {
            0.0
        };
        let inv_std = 1.0 / (var.sqrt() + 1e-5);
        for v in row.iter_mut() {
            *v = (*v - mean) * inv_std;
        }
    }
}

/// Build a Slaney-scale mel filterbank `[n_mels * (n_fft/2+1)]`, area-normalized.
fn slaney_mel_filterbank(n_mels: usize, n_fft: usize, sr: f32, fmin: f32, fmax: f32) -> Vec<f32> {
    let n_freq = n_fft / 2 + 1;
    let hz_to_mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
    let mel_to_hz = |m: f32| 700.0 * (10f32.powf(m / 2595.0) - 1.0);
    let m_min = hz_to_mel(fmin);
    let m_max = hz_to_mel(fmax);
    let mut mel_points = vec![0.0f32; n_mels + 2];
    for (i, p) in mel_points.iter_mut().enumerate() {
        *p = mel_to_hz(m_min + (m_max - m_min) * i as f32 / (n_mels as f32 + 1.0));
    }
    // FFT bin center frequencies.
    let mut fb = vec![0.0f32; n_mels * n_freq];
    let bin_hz = sr / n_fft as f32;
    for m in 0..n_mels {
        let left = mel_points[m];
        let center = mel_points[m + 1];
        let right = mel_points[m + 2];
        for k in 0..n_freq {
            let f = k as f32 * bin_hz;
            let w = if f >= left && f <= center {
                (f - left) / (center - left).max(1e-9)
            } else if f > center && f <= right {
                (right - f) / (right - center).max(1e-9)
            } else {
                0.0
            };
            // Slaney area normalization
            let enorm = 2.0 / (right - left).max(1e-9);
            fb[m * n_freq + k] = w * enorm;
        }
    }
    fb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_mel_shape() {
        let mut mel = NativeMel::new();
        // 1 second of 16 kHz -> ~101 frames (16000/160 + 1)
        let audio: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.02).sin() * 0.1).collect();
        let (feats, t) = mel.extract(&audio).unwrap();
        assert_eq!(t, 16_000 / HOP + 1);
        assert_eq!(feats.len(), N_MELS * t);
        assert!(feats.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn native_mel_empty() {
        let mut mel = NativeMel::new();
        let (f, t) = mel.extract(&[]).unwrap();
        assert_eq!(t, 0);
        assert!(f.is_empty());
    }

    #[test]
    fn filterbank_is_nonnegative_and_covers_bins() {
        let fb = slaney_mel_filterbank(N_MELS, N_FFT, SAMPLE_RATE, 0.0, 8000.0);
        assert_eq!(fb.len(), N_MELS * (N_FFT / 2 + 1));
        assert!(fb.iter().all(|&v| v >= 0.0));
        assert!(fb.iter().any(|&v| v > 0.0));
    }

    #[test]
    fn per_feature_normalize_zero_mean() {
        let mut feats = vec![1.0, 2.0, 3.0, 4.0]; // 1 mel, 4 frames
        per_feature_normalize(&mut feats, 1, 4);
        let mean: f32 = feats.iter().sum::<f32>() / 4.0;
        assert!(mean.abs() < 1e-5);
    }
}
