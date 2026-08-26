//! Diagnostics: a structured report of the app/runtime/hardware state and a latency-metrics
//! collector. Nothing here logs audio, transcripts, clipboard contents, or keys.

use serde::{Deserialize, Serialize};

use crate::capabilities::Capabilities;
use crate::engine::{Acceleration, Provider};

/// One recorded latency measurement (milliseconds).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct LatencyMetrics {
    /// Time from mic buffer to VAD decision.
    pub vad_ms: f32,
    /// Feature-extraction (mel) time.
    pub feature_ms: f32,
    /// Encoder inference time.
    pub encoder_ms: f32,
    /// Decoder (TDT) time.
    pub decoder_ms: f32,
    /// Text pipeline time.
    pub pipeline_ms: f32,
    /// Total from end-of-audio to final text.
    pub total_ms: f32,
    /// Audio duration processed (seconds).
    pub audio_secs: f32,
}

impl LatencyMetrics {
    /// Real-time factor: processing time / audio duration (lower is faster; < 1 is real-time).
    pub fn rtf(&self) -> f32 {
        if self.audio_secs <= 0.0 {
            0.0
        } else {
            (self.total_ms / 1000.0) / self.audio_secs
        }
    }
}

/// The full diagnostics report surfaced in the UI and `lw diagnose`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiagnosticsReport {
    /// App version string.
    pub app_version: String,
    /// Selected STT engine (model id).
    pub engine: String,
    /// Provider actually in use.
    pub provider: Provider,
    /// Acceleration class actually in use.
    pub acceleration: Acceleration,
    /// Human-readable device string.
    pub device: String,
    /// Model version/id.
    pub model_version: String,
    /// Model directory path.
    pub model_path: String,
    /// ONNX Runtime version string, if known.
    pub runtime_version: Option<String>,
    /// QNN/QAIRT version string, if known.
    pub qnn_version: Option<String>,
    /// Selected audio input device.
    pub audio_device: String,
    /// Hardware/OS capabilities.
    pub capabilities: Capabilities,
    /// Most recent latency metrics, if any.
    pub last_latency: Option<LatencyMetrics>,
    /// Free-form notes (backend-selection log, warnings).
    pub notes: Vec<String>,
}

impl DiagnosticsReport {
    /// A minimal report before an engine is initialized.
    pub fn stub(app_version: impl Into<String>, capabilities: Capabilities) -> Self {
        Self {
            app_version: app_version.into(),
            engine: String::new(),
            provider: Provider::OnnxCpu,
            acceleration: Acceleration::Cpu,
            device: String::new(),
            model_version: String::new(),
            model_path: String::new(),
            runtime_version: None,
            qnn_version: None,
            audio_device: String::new(),
            capabilities,
            last_latency: None,
            notes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtf_is_computed() {
        let m = LatencyMetrics {
            total_ms: 100.0,
            audio_secs: 5.0,
            ..Default::default()
        };
        assert!((m.rtf() - 0.02).abs() < 1e-6);
    }

    #[test]
    fn rtf_zero_when_no_audio() {
        assert_eq!(LatencyMetrics::default().rtf(), 0.0);
    }

    #[test]
    fn report_serializes() {
        let r = DiagnosticsReport::stub("0.1.0", Capabilities::unknown());
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("0.1.0"));
    }
}
