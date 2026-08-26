//! Sample-rate conversion to 16 kHz mono using a high-quality sinc resampler (`rubato`).
//!
//! Resampling runs off the audio callback thread — on the inference worker after a recording ends,
//! or in a batch pass over a buffer. It is never called from the real-time capture callback.

use rubato::{
    Resampler as _, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::{Error, Result};

/// A one-shot mono resampler from `from_hz` to `to_hz`.
pub struct Resampler {
    from_hz: u32,
    to_hz: u32,
    inner: Option<SincFixedIn<f32>>,
}

impl Resampler {
    /// Create a resampler. If `from == to`, `process` is a cheap clone.
    pub fn new(from_hz: u32, to_hz: u32) -> Result<Self> {
        if from_hz == 0 || to_hz == 0 {
            return Err(Error::Audio("sample rate must be non-zero".into()));
        }
        Ok(Self { from_hz, to_hz, inner: None })
    }

    /// Resample the whole `input` slice, returning the converted samples.
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<f32>> {
        if self.from_hz == self.to_hz {
            return Ok(input.to_vec());
        }
        if input.is_empty() {
            return Ok(Vec::new());
        }
        let ratio = self.to_hz as f64 / self.from_hz as f64;
        let params = SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::BlackmanHarris2,
        };
        // Process the whole clip in one chunk (FixedIn with chunk = input length).
        let mut resampler = SincFixedIn::<f32>::new(ratio, 2.0, params, input.len(), 1)
            .map_err(|e| Error::Audio(format!("resampler init: {e}")))?;
        let out = resampler
            .process(&[input], None)
            .map_err(|e| Error::Audio(format!("resample: {e}")))?;
        self.inner = Some(resampler);
        Ok(out.into_iter().next().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_when_same_rate() {
        let mut r = Resampler::new(16_000, 16_000).unwrap();
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        assert_eq!(r.process(&input).unwrap(), input);
    }

    #[test]
    fn downsample_48k_to_16k_length() {
        let mut r = Resampler::new(48_000, 16_000).unwrap();
        // 4800 samples @48k = 0.1s -> ~1600 @16k
        let input: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.02).sin()).collect();
        let out = r.process(&input).unwrap();
        let expected = 1600i64;
        assert!(
            (out.len() as i64 - expected).abs() <= 200,
            "got {} expected ~{}",
            out.len(),
            expected
        );
    }

    #[test]
    fn empty_input_ok() {
        let mut r = Resampler::new(44_100, 16_000).unwrap();
        assert!(r.process(&[]).unwrap().is_empty());
    }

    #[test]
    fn zero_rate_rejected() {
        assert!(Resampler::new(0, 16_000).is_err());
    }
}
