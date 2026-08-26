//! Audio primitives: the canonical buffer, a lock-light ring buffer for capture,
//! sample-format conversion, and a resampler wrapper.
//!
//! The whole pipeline works in **16 kHz mono `f32`** in `[-1.0, 1.0]`. Capture code converts the
//! device's native format to this canonical form as early as possible; nothing downstream deals
//! with sample formats or channel counts.

mod resampler;
mod ring;

pub use resampler::Resampler;
pub use ring::RingBuffer;

/// The model / pipeline sample rate. Parakeet and Silero VAD both require 16 kHz.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// An owned buffer of 16 kHz mono `f32` samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioBuffer {
    /// Samples in `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
    /// Sample rate in Hz. Almost always [`TARGET_SAMPLE_RATE`].
    pub sample_rate: u32,
}

impl AudioBuffer {
    /// A new empty buffer at the target sample rate.
    pub fn empty() -> Self {
        Self {
            samples: Vec::new(),
            sample_rate: TARGET_SAMPLE_RATE,
        }
    }

    /// Wrap samples known to be at `sample_rate`.
    pub fn new(samples: Vec<f32>, sample_rate: u32) -> Self {
        Self { samples, sample_rate }
    }

    /// Duration in seconds.
    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.samples.len() as f32 / self.sample_rate as f32
        }
    }

    /// Number of samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Root-mean-square level of the whole buffer, in `[0, 1]`.
    pub fn rms(&self) -> f32 {
        rms(&self.samples)
    }

    /// Resample to 16 kHz mono if needed, returning a canonical buffer.
    pub fn to_target(&self) -> crate::Result<AudioBuffer> {
        if self.sample_rate == TARGET_SAMPLE_RATE {
            return Ok(self.clone());
        }
        let mut r = Resampler::new(self.sample_rate, TARGET_SAMPLE_RATE)?;
        let out = r.process(&self.samples)?;
        Ok(AudioBuffer::new(out, TARGET_SAMPLE_RATE))
    }
}

/// Root-mean-square of a slice, in `[0, 1]` for well-formed audio.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&x| (x as f64) * (x as f64)).sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// Downmix interleaved multi-channel `f32` frames to mono by averaging channels.
pub fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    if ch == 1 {
        return interleaved.to_vec();
    }
    let frames = interleaved.len() / ch;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let base = f * ch;
        let mut acc = 0.0f32;
        for c in 0..ch {
            acc += interleaved[base + c];
        }
        out.push(acc / ch as f32);
    }
    out
}

/// Convert an interleaved `i16` buffer to `f32` in `[-1, 1]`.
pub fn i16_to_f32(input: &[i16]) -> Vec<f32> {
    input.iter().map(|&s| s as f32 / 32768.0).collect()
}

/// Convert an interleaved `u16` buffer to `f32` in `[-1, 1]`.
pub fn u16_to_f32(input: &[u16]) -> Vec<f32> {
    input.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_channels() {
        // 2 frames, stereo: (0.0,1.0), (0.5,-0.5)
        let interleaved = [0.0, 1.0, 0.5, -0.5];
        let mono = downmix_to_mono(&interleaved, 2);
        assert_eq!(mono, vec![0.5, 0.0]);
    }

    #[test]
    fn downmix_mono_is_identity() {
        let s = [0.1, -0.2, 0.3];
        assert_eq!(downmix_to_mono(&s, 1), s.to_vec());
    }

    #[test]
    fn i16_conversion_range() {
        assert!((i16_to_f32(&[i16::MIN])[0] - -1.0).abs() < 1e-6);
        assert!((i16_to_f32(&[0])[0]).abs() < 1e-6);
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[0.0; 100]), 0.0);
    }

    #[test]
    fn rms_of_full_scale() {
        let ones = [1.0f32; 100];
        assert!((rms(&ones) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn duration_matches_rate() {
        let b = AudioBuffer::new(vec![0.0; 16_000], 16_000);
        assert!((b.duration_secs() - 1.0).abs() < 1e-6);
    }
}
