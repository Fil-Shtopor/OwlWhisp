//! Sample-rate conversion to 16 kHz mono using a high-quality sinc resampler (`rubato`).
//!
//! Resampling runs off the audio callback thread — on the inference worker after a recording ends,
//! or in a batch pass over a buffer. It is never called from the real-time capture callback.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Resampler as _, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::{Error, Result};

/// How many frames the resampler is handed at a time.
///
/// An implementation detail of the resampler, not of the clip: `process_all` loops until the whole
/// input is consumed, so this only decides how much scratch it works in. A clip-sized chunk would
/// make it allocate a buffer as large as the recording for no benefit.
const CHUNK: usize = 1024;

/// A one-shot mono resampler from `from_hz` to `to_hz`.
pub struct Resampler {
    from_hz: u32,
    to_hz: u32,
}

impl Resampler {
    /// Create a resampler. If `from == to`, `process` is a cheap clone.
    pub fn new(from_hz: u32, to_hz: u32) -> Result<Self> {
        if from_hz == 0 || to_hz == 0 {
            return Err(Error::Audio("sample rate must be non-zero".into()));
        }
        Ok(Self { from_hz, to_hz })
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
            // `Some` rather than a bare number since rubato 5: `None` asks it to derive the cutoff
            // from the sinc length. Kept explicit at the value this has always used, so the
            // filter is the same one that produced every word rate measured so far.
            f_cutoff: Some(0.95),
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::BlackmanHarris2,
        };
        let mut resampler = Async::<f32>::new_sinc(ratio, 2.0, &params, CHUNK, 1, FixedAsync::Input)
            .map_err(|e| Error::Audio(format!("resampler init: {e}")))?;
        // One channel, so interleaved and sequential are the same arrangement.
        let adapter = InterleavedSlice::new(input, 1, input.len())
            .map_err(|e| Error::Audio(format!("resampler input: {e}")))?;
        // `process_all` rather than a single `process`: it feeds the whole clip through in chunks
        // and trims the resampler's own delay off the front. The old one-chunk call did not, so
        // every converted clip began with about sixty samples of the filter warming up.
        let out = resampler
            .process_all(&adapter, input.len(), None)
            .map_err(|e| Error::Audio(format!("resample: {e}")))?;
        Ok(out.take_data())
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
    fn a_tone_survives_the_conversion_as_the_same_tone() {
        // Length alone says nothing about whether the samples are audio. A resampler that
        // returned silence, or the wrong rate, or a buffer of filter warm-up, would pass a length
        // check and hand the speech model something it cannot read. So: a 1 kHz tone at 48 kHz,
        // converted to 16 kHz, has to come back as a 1 kHz tone at full amplitude.
        let rate_in = 48_000;
        let seconds = 0.5;
        let input: Vec<f32> = (0..(rate_in as f32 * seconds) as usize)
            .map(|i| (i as f32 / rate_in as f32 * 1000.0 * std::f32::consts::TAU).sin())
            .collect();

        let mut r = Resampler::new(rate_in, 16_000).unwrap();
        let out = r.process(&input).unwrap();

        // Frequency, by counting zero crossings: 1 kHz for half a second is 500 cycles, so a
        // thousand crossings, give or take the ends.
        let crossings = out.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!(
            (crossings as i64 - 1000).abs() <= 8,
            "counted {crossings} zero crossings, expected about 1000 -- the tone changed pitch"
        );

        // Amplitude, away from the edges, where a filter's tails live.
        let peak = out[200..out.len() - 200]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            (0.95..=1.05).contains(&peak),
            "peak amplitude {peak}, expected about 1.0"
        );

        // And it starts as a tone rather than with the resampler warming up, which is what
        // `process_all` trims and the previous one-shot call did not.
        let first = out[..64].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(first > 0.5, "the clip opens at {first}, not at signal level");
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
