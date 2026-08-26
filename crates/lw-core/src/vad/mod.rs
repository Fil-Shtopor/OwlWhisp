//! Voice-activity detection: a model-agnostic trait plus the end-of-utterance state machine.
//!
//! The [`Vad`] trait is implemented by a concrete detector (e.g. Silero in `lw-vad-silero`). It
//! consumes fixed-size 16 kHz frames and returns a speech probability per frame. The
//! [`EndpointDetector`] turns that probability stream into speech segments and end-of-utterance
//! events, with hysteresis and dictation-tuned timing. Keeping the two apart means the STT model
//! and the VAD model are fully independent.

mod endpoint;

pub use endpoint::{EndpointConfig, EndpointDetector, EndpointEvent, SpeechSegment};

/// Frame size Silero VAD expects at 16 kHz (32 ms hop). The detector feeds exactly this many
/// samples per step (plus internal context handled by the model implementation).
pub const VAD_FRAME_SIZE: usize = 512;

/// A voice-activity detector operating on 16 kHz mono `f32` frames.
pub trait Vad: Send {
    /// Feed one [`VAD_FRAME_SIZE`]-sample frame; return the speech probability in `[0, 1]`.
    fn process_frame(&mut self, frame: &[f32]) -> crate::Result<f32>;

    /// Reset internal state (call at the start of each recording).
    fn reset(&mut self);
}

/// A trivial energy-based VAD used as a dependency-free fallback and in tests.
///
/// It is intentionally simple: RMS above a fixed threshold counts as speech. It is not as accurate
/// as Silero but keeps the pipeline working when no model is present.
pub struct EnergyVad {
    threshold: f32,
}

impl EnergyVad {
    /// New energy VAD with an RMS `threshold` (typical: 0.01–0.03).
    pub fn new(threshold: f32) -> Self {
        Self { threshold }
    }
}

impl Default for EnergyVad {
    fn default() -> Self {
        Self::new(0.015)
    }
}

impl Vad for EnergyVad {
    fn process_frame(&mut self, frame: &[f32]) -> crate::Result<f32> {
        let level = crate::audio::rms(frame);
        // Map RMS to a pseudo-probability with a soft knee around the threshold.
        let p = (level / (self.threshold * 2.0)).clamp(0.0, 1.0);
        Ok(p)
    }
    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_vad_silence_low_speech_high() {
        let mut v = EnergyVad::new(0.02);
        let silence = [0.0f32; VAD_FRAME_SIZE];
        let loud: Vec<f32> = (0..VAD_FRAME_SIZE).map(|i| if i % 2 == 0 { 0.3 } else { -0.3 }).collect();
        assert!(v.process_frame(&silence).unwrap() < 0.1);
        assert!(v.process_frame(&loud).unwrap() > 0.5);
    }
}
