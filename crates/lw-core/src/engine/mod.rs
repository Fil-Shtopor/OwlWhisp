//! The speech-engine abstraction and its supporting types.
//!
//! The core never names a concrete model. It holds `Box<dyn SpeechEngine>` selected by an
//! [`EngineRegistry`] according to a [`BackendPreference`]. Every engine must report — truthfully —
//! which provider and device it is actually running on, so the Diagnostics screen can never claim
//! NPU acceleration that did not happen.

use serde::{Deserialize, Serialize};
use std::fmt;

mod registry;
pub use registry::{BackendCandidate, BackendPreference, EngineRegistry, SelectionOutcome};

/// The compute backend actually in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// ONNX Runtime CPU execution provider.
    OnnxCpu,
    /// ONNX Runtime QNN execution provider (Qualcomm Hexagon).
    QnnHtp,
    /// ONNX Runtime CoreML execution provider (Apple).
    CoreMl,
    /// ONNX Runtime DirectML execution provider.
    DirectMl,
    /// ONNX Runtime WebGPU execution provider (vendor-neutral: D3D12 / Vulkan / Metal).
    Gpu,
    /// ONNX Runtime CUDA execution provider (NVIDIA).
    Cuda,
    /// ONNX Runtime TensorRT execution provider (NVIDIA).
    TensorRt,
    /// ONNX Runtime OpenVINO execution provider (Intel CPU / GPU / NPU).
    OpenVino,
    /// ONNX Runtime Vitis AI execution provider (AMD XDNA NPU).
    VitisAi,
    /// A non-ORT engine (e.g. sherpa-onnx) on CPU.
    NativeCpu,
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Provider::OnnxCpu => "ONNX Runtime CPU",
            Provider::QnnHtp => "QNN",
            Provider::CoreMl => "CoreML",
            Provider::DirectMl => "DirectML",
            Provider::Gpu => "WebGPU",
            Provider::Cuda => "CUDA",
            Provider::TensorRt => "TensorRT",
            Provider::OpenVino => "OpenVINO",
            Provider::VitisAi => "Vitis AI",
            Provider::NativeCpu => "Native CPU",
        };
        f.write_str(s)
    }
}

/// The acceleration class of a provider — the one-word honest answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Acceleration {
    /// Runs on the CPU.
    Cpu,
    /// Runs on a neural processing unit (Hexagon HTP).
    Npu,
    /// Runs on a GPU.
    Gpu,
    /// Runs on Apple Neural Engine.
    Ane,
}

impl fmt::Display for Acceleration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Acceleration::Cpu => "CPU",
            Acceleration::Npu => "NPU",
            Acceleration::Gpu => "GPU",
            Acceleration::Ane => "ANE",
        };
        f.write_str(s)
    }
}

/// Human-readable device description, e.g. "Snapdragon X2 Elite HTP (V81)".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Short device name.
    pub name: String,
    /// Optional detail (arch, thread count).
    pub detail: Option<String>,
}

impl DeviceInfo {
    /// Construct with a name only.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            detail: None,
        }
    }
    /// Construct with a name and a detail line.
    pub fn with_detail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            detail: Some(detail.into()),
        }
    }
}

/// A supported language, by BCP-47-ish code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Language(pub &'static str);

/// One recognized token with a time span (from TDT durations).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Token {
    /// The token text (already detokenized; may contain a leading space).
    pub text: String,
    /// Start time in seconds.
    pub start: f32,
    /// End time in seconds.
    pub end: f32,
}

/// The result of transcription.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// The full text.
    pub text: String,
    /// Optional per-token timing.
    pub tokens: Vec<Token>,
    /// Detected language code, if the engine reports one.
    pub language: Option<String>,
}

impl Transcript {
    /// A transcript from plain text with no timing.
    pub fn from_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tokens: Vec::new(),
            language: None,
        }
    }
}

/// Result of a `health_check`: did the claimed device actually run a trivial inference?
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthReport {
    /// Whether a probe inference succeeded on the claimed provider.
    pub ok: bool,
    /// The provider the probe actually executed on.
    pub provider: Provider,
    /// Probe latency in milliseconds, if it ran.
    pub probe_latency_ms: Option<f32>,
    /// A message (error detail on failure, device string on success).
    pub message: String,
}

/// Context passed to `initialize`: paths and tuning knobs an engine may need.
#[derive(Clone, Debug, Default)]
pub struct EngineInitContext {
    /// Directory containing the model files.
    pub model_dir: std::path::PathBuf,
    /// Directory for caches (e.g. compiled NPU context binaries).
    pub cache_dir: std::path::PathBuf,
    /// CPU thread budget for CPU backends.
    pub cpu_threads: usize,
}

/// A streaming transcription handle (for engines that support partials).
pub trait TranscribeStream: Send {
    /// Feed more audio; return a partial transcript if one is available.
    fn push(&mut self, audio: &crate::audio::AudioBuffer) -> crate::Result<Option<Transcript>>;
    /// Finish and return the final transcript.
    fn finish(self: Box<Self>) -> crate::Result<Transcript>;
}

/// The engine abstraction. Implementors live in the `lw-engine-*` crates.
pub trait SpeechEngine: Send {
    /// Stable model identifier, e.g. `"parakeet-tdt-0.6b-v3"`.
    fn backend_name(&self) -> &str;
    /// The provider actually in use after initialization.
    fn provider(&self) -> Provider;
    /// The device actually in use.
    fn device(&self) -> DeviceInfo;
    /// The acceleration class actually in use.
    fn acceleration(&self) -> Acceleration;
    /// Languages the model supports.
    fn supported_languages(&self) -> &[Language];
    /// Whether streaming (partial results) is supported.
    fn supports_streaming(&self) -> bool;

    /// Load the model and prepare sessions. Idempotent.
    fn initialize(&mut self, ctx: &EngineInitContext) -> crate::Result<()>;
    /// Run a trivial probe inference on the claimed device and report honestly.
    fn health_check(&mut self) -> HealthReport;
    /// Transcribe a whole utterance.
    fn transcribe(&mut self, audio: &crate::audio::AudioBuffer) -> crate::Result<Transcript>;
    /// Begin a streaming session (default: unsupported).
    fn start_stream(&mut self) -> crate::Result<Box<dyn TranscribeStream>> {
        Err(crate::Error::Unavailable(
            "streaming not supported by this engine".into(),
        ))
    }
    /// Notes recorded while selecting a backend, in order.
    ///
    /// This is how a fallback becomes visible instead of silent: when an engine asks for the NPU
    /// and ends up on the CPU, the reason belongs somewhere the user can read it, not only in a
    /// log. Engines with nothing to say return an empty slice.
    fn notes(&self) -> &[String] {
        &[]
    }

    /// Release resources.
    fn shutdown(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_and_acceleration_display() {
        assert_eq!(Provider::QnnHtp.to_string(), "QNN");
        assert_eq!(Acceleration::Npu.to_string(), "NPU");
    }

    #[test]
    fn transcript_from_text() {
        let t = Transcript::from_text("hello");
        assert_eq!(t.text, "hello");
        assert!(t.tokens.is_empty());
    }
}
