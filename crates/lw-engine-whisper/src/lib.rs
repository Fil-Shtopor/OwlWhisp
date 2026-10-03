//! # lw-engine-whisper
//!
//! Whisper backend adapter for OwlWhisp. This crate exists to prove the [`SpeechEngine`]
//! abstraction is model-agnostic: a Whisper engine can be dropped in behind the same trait the
//! Parakeet engine implements, with no change to the core, CLI, or UI.
//!
//! v1 ships the abstraction and a not-yet-wired implementation. A real Whisper backend
//! (whisper.cpp / transcribe-cpp GGUF, or an ONNX Whisper) plugs in at [`WhisperEngine::transcribe`]
//! and [`WhisperEngine::initialize`]. Until then it reports itself unavailable via `health_check`,
//! so backend selection skips it rather than pretending it works.
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use std::path::PathBuf;

use lw_core::audio::AudioBuffer;
use lw_core::engine::{
    Acceleration, DeviceInfo, EngineInitContext, HealthReport, Language, Provider, SpeechEngine, Transcript,
};

/// Configuration for the Whisper engine.
#[derive(Clone, Debug, Default)]
pub struct WhisperConfig {
    /// Directory containing the Whisper model files (GGUF or ONNX).
    pub model_dir: PathBuf,
    /// Model file name.
    pub model_file: String,
}

/// A Whisper speech engine adapter.
///
/// Implements [`SpeechEngine`] so it is interchangeable with the Parakeet engine. The concrete
/// inference backend is not wired in v1; `initialize`/`health_check` report unavailability.
pub struct WhisperEngine {
    config: WhisperConfig,
    ready: bool,
}

impl WhisperEngine {
    /// Create a Whisper engine with the given config.
    pub fn new(config: WhisperConfig) -> Self {
        Self { config, ready: false }
    }
}

const WHISPER_LANGUAGES: &[Language] = &[Language("en"), Language("multi")];

impl SpeechEngine for WhisperEngine {
    fn backend_name(&self) -> &str {
        "whisper"
    }
    fn provider(&self) -> Provider {
        Provider::OnnxCpu
    }
    fn device(&self) -> DeviceInfo {
        DeviceInfo::new("CPU")
    }
    fn acceleration(&self) -> Acceleration {
        Acceleration::Cpu
    }
    fn supported_languages(&self) -> &[Language] {
        WHISPER_LANGUAGES
    }
    fn supports_streaming(&self) -> bool {
        false
    }

    fn initialize(&mut self, ctx: &EngineInitContext) -> lw_core::Result<()> {
        if !ctx.model_dir.as_os_str().is_empty() {
            self.config.model_dir = ctx.model_dir.clone();
        }
        // A real backend (whisper.cpp/transcribe-cpp/ONNX) is wired here in a future revision.
        self.ready = false;
        Err(lw_core::Error::Unavailable(
            "Whisper backend is not built into this release; use the Parakeet engine".into(),
        ))
    }

    fn health_check(&mut self) -> HealthReport {
        HealthReport {
            ok: false,
            provider: Provider::OnnxCpu,
            probe_latency_ms: None,
            message: "Whisper backend not wired in this build".into(),
        }
    }

    fn transcribe(&mut self, _audio: &AudioBuffer) -> lw_core::Result<Transcript> {
        Err(lw_core::Error::Unavailable(
            "Whisper backend not available".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implements_speech_engine_and_reports_unavailable() {
        let mut e = WhisperEngine::new(WhisperConfig::default());
        // Trait object usable — proves the abstraction holds.
        let obj: &mut dyn SpeechEngine = &mut e;
        assert_eq!(obj.backend_name(), "whisper");
        assert!(!obj.health_check().ok);
        assert!(obj.transcribe(&AudioBuffer::empty()).is_err());
    }
}
