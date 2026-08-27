//! Shared helpers for the integration tests. Not a test target of its own.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use lw_core::audio::{AudioBuffer, downmix_to_mono};

/// Word error rate of `hypothesis` against `reference`, after lowercasing and stripping
/// punctuation. Same normalization as `lw-engine-parakeet`'s fixture test, so the numbers the two
/// engines print are directly comparable.
pub fn wer(reference: &str, hypothesis: &str) -> f32 {
    let norm = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c.is_whitespace() {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .map(str::to_string)
            .collect()
    };
    let r = norm(reference);
    let h = norm(hypothesis);
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    let mut cur = vec![0usize; h.len() + 1];
    for (i, rw) in r.iter().enumerate() {
        cur[0] = i + 1;
        for (j, hw) in h.iter().enumerate() {
            let cost = if rw == hw { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[h.len()] as f32 / r.len() as f32
}

/// Read a WAV file into the canonical 16 kHz-mono-`f32` buffer.
pub fn load_wav(path: &Path) -> AudioBuffer {
    let mut reader = hound::WavReader::open(path).unwrap_or_else(|e| panic!("open {path:?}: {e}"));
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.unwrap() as f32 / max).collect()
        }
    };
    AudioBuffer::new(downmix_to_mono(&interleaved, spec.channels), spec.sample_rate)
}

/// The workspace root (this crate is at `crates/lw-engine-sherpa`).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// A WAV under `tests/fixtures/audio` at the repo root.
pub fn fixture(name: &str) -> PathBuf {
    repo_root().join("tests/fixtures/audio").join(name)
}

/// An env-provided path, or `None` (so the test can skip loudly instead of failing).
pub fn env_dir(var: &str) -> Option<PathBuf> {
    let raw = std::env::var(var).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

/// Print a one-line performance summary and return the real-time factor.
pub fn report(label: &str, audio_secs: f32, elapsed: std::time::Duration, text: &str) -> f32 {
    let wall = elapsed.as_secs_f32();
    let rtf = if audio_secs > 0.0 {
        wall / audio_secs
    } else {
        f32::NAN
    };
    eprintln!("--- {label} ---");
    eprintln!("transcript: {text}");
    eprintln!("audio: {audio_secs:.2} s   wall: {wall:.3} s   RTF: {rtf:.3}");
    rtf
}
