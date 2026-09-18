//! The [`SpeechEngine`] implementation on top of sherpa-onnx's offline recognizer.

use std::path::Path;
use std::time::Instant;

use lw_core::audio::{AudioBuffer, TARGET_SAMPLE_RATE};
use lw_core::engine::{
    Acceleration, DeviceInfo, EngineInitContext, HealthReport, Language, Provider, SpeechEngine, Token,
    Transcript,
};
use sherpa_onnx::{
    OfflineMoonshineModelConfig, OfflineOmnilingualAsrCtcModelConfig, OfflineParaformerModelConfig,
    OfflineQwen3ASRModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineRecognizerResult,
    OfflineSenseVoiceModelConfig, OfflineTransducerModelConfig, OfflineWhisperModelConfig,
};

use crate::config::SherpaConfig;
use crate::detect::{ModelFiles, ModelKind, detect_in_dir};
use crate::error::{Error, Result};
use crate::lang;

/// A portable CPU speech engine backed by sherpa-onnx.
///
/// One instance drives one model. The model family is detected from the files in
/// [`SherpaConfig::model_dir`] (see [`crate::detect`]), so the same engine type serves Whisper,
/// Moonshine, SenseVoice, Paraformer and offline transducers such as Parakeet TDT.
///
/// ```no_run
/// use lw_core::audio::AudioBuffer;
/// use lw_core::engine::SpeechEngine;
/// use lw_engine_sherpa::{SherpaConfig, SherpaEngine};
///
/// let mut engine = SherpaEngine::open(SherpaConfig::new("/models/sherpa-onnx-whisper-tiny.en"))?;
/// let transcript = engine.transcribe(&AudioBuffer::new(vec![0.0; 16_000], 16_000))?;
/// println!("{} via {}", transcript.text, engine.backend_name());
/// # Ok::<(), lw_core::Error>(())
/// ```
pub struct SherpaEngine {
    config: SherpaConfig,
    /// Kept as an owned `String` because [`SpeechEngine::backend_name`] returns a borrow, and the
    /// name only becomes known once the model directory has been inspected.
    backend_name: String,
    inner: Option<Inner>,
}

struct Inner {
    recognizer: OfflineRecognizer,
    files: ModelFiles,
    languages: &'static [Language],
    threads: usize,
    /// The language actually handed to sherpa-onnx, echoed back on the transcript.
    language: Option<String>,
}

impl SherpaEngine {
    /// Create an engine without loading anything. Call
    /// [`initialize`](SpeechEngine::initialize) before transcribing.
    pub fn new(config: SherpaConfig) -> Self {
        Self {
            config,
            backend_name: "sherpa-onnx".to_string(),
            inner: None,
        }
    }

    /// Create *and* initialize an engine in one step.
    pub fn open(config: SherpaConfig) -> lw_core::Result<Self> {
        let ctx = EngineInitContext {
            model_dir: config.model_dir.clone(),
            cache_dir: std::path::PathBuf::new(),
            cpu_threads: config.num_threads,
        };
        let mut engine = Self::new(config);
        engine.initialize(&ctx)?;
        Ok(engine)
    }

    /// The configuration in use, including any values `initialize` took from the context.
    pub fn config(&self) -> &SherpaConfig {
        &self.config
    }

    /// The detected model family, once initialized.
    pub fn model_kind(&self) -> Option<ModelKind> {
        self.inner.as_ref().map(|i| i.files.kind())
    }

    /// The concrete model files in use, once initialized.
    pub fn model_files(&self) -> Option<&ModelFiles> {
        self.inner.as_ref().map(|i| &i.files)
    }

    /// Whether the model has been loaded.
    pub fn is_initialized(&self) -> bool {
        self.inner.is_some()
    }

    fn load(&mut self) -> Result<()> {
        self.config.validate()?;
        let files = detect_in_dir(
            &self.config.model_dir,
            self.config.prefer_quantized,
            self.config.model_kind,
        )?;
        files.verify_exists()?;

        let threads = self.config.effective_threads();
        let language = self.config.language_for(&files)?;
        let recognizer = build_recognizer(&self.config, &files, threads, language.as_deref())?;

        tracing::info!(
            kind = %files.kind(),
            dir = %self.config.model_dir.display(),
            threads,
            decoding = self.config.effective_decoding_method(),
            language = language.as_deref().unwrap_or("-"),
            "sherpa-onnx offline recognizer ready"
        );

        self.backend_name = files.kind().as_str().to_string();
        self.inner = Some(Inner {
            recognizer,
            languages: lang::languages_for(&files),
            files,
            threads,
            language,
        });
        Ok(())
    }
}

impl SpeechEngine for SherpaEngine {
    fn backend_name(&self) -> &str {
        &self.backend_name
    }

    fn provider(&self) -> Provider {
        Provider::NativeCpu
    }

    fn device(&self) -> DeviceInfo {
        let threads = self
            .inner
            .as_ref()
            .map_or_else(|| self.config.effective_threads(), |i| i.threads);
        let plural = if threads == 1 { "thread" } else { "threads" };
        let detail = match self.inner.as_ref() {
            Some(i) => format!(
                "sherpa-onnx {} ({}), bundled ONNX Runtime CPU provider",
                i.files.kind(),
                self.config.effective_decoding_method()
            ),
            None => "sherpa-onnx (not initialized)".to_string(),
        };
        DeviceInfo::with_detail(format!("CPU ({threads} {plural})"), detail)
    }

    fn acceleration(&self) -> Acceleration {
        Acceleration::Cpu
    }

    fn supported_languages(&self) -> &[Language] {
        self.inner.as_ref().map_or(&[], |i| i.languages)
    }

    fn supports_streaming(&self) -> bool {
        // sherpa-onnx does have streaming recognizers, but only for a different set of models
        // (streaming Zipformer / Paraformer). Every family this engine loads is offline.
        false
    }

    fn initialize(&mut self, ctx: &EngineInitContext) -> lw_core::Result<()> {
        if !ctx.model_dir.as_os_str().is_empty() {
            self.config.model_dir = ctx.model_dir.clone();
        }
        if ctx.cpu_threads > 0 {
            self.config.num_threads = ctx.cpu_threads;
        }
        // Idempotent: reloading a 1.5 GB Whisper graph because the caller called `initialize`
        // twice would be an expensive surprise.
        if let Some(inner) = &self.inner
            && inner.threads == self.config.effective_threads()
        {
            return Ok(());
        }
        self.inner = None;
        self.load().map_err(lw_core::Error::from)
    }

    fn health_check(&mut self) -> HealthReport {
        let device = self.device();
        if self.inner.is_none() {
            return HealthReport {
                ok: false,
                provider: Provider::NativeCpu,
                probe_latency_ms: None,
                message: "engine not initialized".to_string(),
            };
        }
        // Half a second of digital silence: enough to exercise the feature extractor, the encoder
        // and the decode loop without waiting on a real utterance.
        let probe = AudioBuffer::new(vec![0.0f32; TARGET_SAMPLE_RATE as usize / 2], TARGET_SAMPLE_RATE);
        let started = Instant::now();
        match self.transcribe(&probe) {
            Ok(t) => {
                let ms = started.elapsed().as_secs_f32() * 1000.0;
                HealthReport {
                    ok: true,
                    provider: Provider::NativeCpu,
                    probe_latency_ms: Some(ms),
                    message: format!(
                        "{} on {} — 0.5 s silent probe decoded in {ms:.0} ms ({} chars out)",
                        self.backend_name,
                        device.name,
                        t.text.chars().count()
                    ),
                }
            }
            Err(e) => HealthReport {
                ok: false,
                provider: Provider::NativeCpu,
                probe_latency_ms: None,
                message: format!("probe failed on {}: {e}", device.name),
            },
        }
    }

    fn transcribe(&mut self, audio: &AudioBuffer) -> lw_core::Result<Transcript> {
        let inner = self.inner.as_ref().ok_or(Error::NotInitialized)?;
        let audio = audio.to_target()?;
        if audio.samples.is_empty() {
            return Ok(Transcript::default());
        }
        let stream = inner.recognizer.create_stream();
        stream.accept_waveform(TARGET_SAMPLE_RATE as i32, &audio.samples);
        inner.recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or_else(|| Error::Sherpa("the recognizer returned no result".into()))?;
        Ok(to_transcript(result, inner.language.as_deref()))
    }

    fn shutdown(&mut self) {
        self.inner = None;
        self.backend_name = "sherpa-onnx".to_string();
    }
}

/// Turn sherpa's JSON result into an [`lw_core`] transcript, keeping timestamps when the model
/// produced them (transducers and Moonshine always do; Whisper only with token timestamps on).
fn to_transcript(result: OfflineRecognizerResult, language: Option<&str>) -> Transcript {
    let mut transcript = Transcript::from_text(result.text.trim());
    transcript.language = language.filter(|l| *l != "auto").map(str::to_string);

    if let Some(starts) = result
        .timestamps
        .as_ref()
        .filter(|t| t.len() == result.tokens.len())
    {
        transcript.tokens = result
            .tokens
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let start = starts[i];
                let end = match result.durations.as_ref() {
                    Some(d) if d.len() == starts.len() => start + d[i],
                    // Without durations, a token runs until the next one starts.
                    _ => starts.get(i + 1).copied().unwrap_or(start),
                };
                Token {
                    text: text.clone(),
                    start,
                    end: end.max(start),
                }
            })
            .collect();
    }
    transcript
}

fn path_str(p: &Path) -> Result<String> {
    p.to_str().map(str::to_string).ok_or_else(|| {
        Error::Config(format!(
            "model path is not valid UTF-8, which the sherpa-onnx C API requires: {}",
            p.display()
        ))
    })
}

fn build_recognizer(
    config: &SherpaConfig,
    files: &ModelFiles,
    threads: usize,
    language: Option<&str>,
) -> Result<OfflineRecognizer> {
    let mut c = OfflineRecognizerConfig {
        decoding_method: Some(config.effective_decoding_method().to_string()),
        max_active_paths: config.max_active_paths,
        ..OfflineRecognizerConfig::default()
    };
    c.model_config.num_threads = threads as i32;
    c.model_config.debug = config.debug;
    c.model_config.provider = Some("cpu".to_string());
    c.model_config.model_type = config.model_type.clone();

    match files {
        ModelFiles::Whisper {
            encoder,
            decoder,
            tokens,
            ..
        } => {
            c.model_config.whisper = OfflineWhisperModelConfig {
                encoder: Some(path_str(encoder)?),
                decoder: Some(path_str(decoder)?),
                language: language.map(str::to_string),
                task: Some("transcribe".to_string()),
                // -1 asks sherpa for its per-model default tail padding.
                tail_paddings: -1,
                enable_token_timestamps: config.whisper_token_timestamps,
                enable_segment_timestamps: false,
            };
            c.model_config.tokens = Some(path_str(tokens)?);
        }
        ModelFiles::MoonshineV1 {
            preprocessor,
            encoder,
            uncached_decoder,
            cached_decoder,
            tokens,
        } => {
            c.model_config.moonshine = OfflineMoonshineModelConfig {
                preprocessor: Some(path_str(preprocessor)?),
                encoder: Some(path_str(encoder)?),
                uncached_decoder: Some(path_str(uncached_decoder)?),
                cached_decoder: Some(path_str(cached_decoder)?),
                merged_decoder: None,
            };
            c.model_config.tokens = Some(path_str(tokens)?);
        }
        ModelFiles::MoonshineV2 {
            encoder,
            merged_decoder,
            tokens,
        } => {
            c.model_config.moonshine = OfflineMoonshineModelConfig {
                preprocessor: None,
                encoder: Some(path_str(encoder)?),
                uncached_decoder: None,
                cached_decoder: None,
                merged_decoder: Some(path_str(merged_decoder)?),
            };
            c.model_config.tokens = Some(path_str(tokens)?);
        }
        ModelFiles::NemoTransducer {
            encoder,
            decoder,
            joiner,
            tokens,
        } => {
            c.model_config.transducer = OfflineTransducerModelConfig {
                encoder: Some(path_str(encoder)?),
                decoder: Some(path_str(decoder)?),
                joiner: Some(path_str(joiner)?),
            };
            c.model_config.tokens = Some(path_str(tokens)?);
        }
        ModelFiles::SenseVoice { model, tokens } => {
            c.model_config.sense_voice = OfflineSenseVoiceModelConfig {
                model: Some(path_str(model)?),
                language: language.map(str::to_string),
                use_itn: config.use_itn,
            };
            c.model_config.tokens = Some(path_str(tokens)?);
        }
        ModelFiles::Paraformer { model, tokens } => {
            c.model_config.paraformer = OfflineParaformerModelConfig {
                model: Some(path_str(model)?),
            };
            c.model_config.tokens = Some(path_str(tokens)?);
        }
        ModelFiles::Omnilingual { model, tokens } => {
            c.model_config.omnilingual = OfflineOmnilingualAsrCtcModelConfig {
                model: Some(path_str(model)?),
            };
            c.model_config.tokens = Some(path_str(tokens)?);
        }
        ModelFiles::Qwen3Asr {
            conv_frontend,
            encoder,
            decoder,
            tokenizer,
        } => {
            c.model_config.qwen3_asr = OfflineQwen3ASRModelConfig {
                conv_frontend: Some(path_str(conv_frontend)?),
                encoder: Some(path_str(encoder)?),
                decoder: Some(path_str(decoder)?),
                tokenizer: Some(path_str(tokenizer)?),
                // Greedy, to match every other family here: sampling would make the same audio
                // transcribe differently between runs, which would make a benchmark meaningless
                // and a dictation unpredictable.
                temperature: 0.0,
                top_p: 1.0,
                ..OfflineQwen3ASRModelConfig::default()
            };
            // No `tokens`: this family carries a BPE vocabulary in its tokenizer directory.
        }
    }

    OfflineRecognizer::create(&c).ok_or_else(|| {
        Error::Sherpa(format!(
            "failed to create an offline recognizer for the {} model in {} \
             (run with debug = true for sherpa-onnx's own diagnostics)",
            files.kind(),
            config.model_dir().display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(
        text: &str,
        tokens: &[&str],
        ts: Option<Vec<f32>>,
        dur: Option<Vec<f32>>,
    ) -> OfflineRecognizerResult {
        OfflineRecognizerResult {
            text: text.to_string(),
            tokens: tokens.iter().map(|s| s.to_string()).collect(),
            timestamps: ts,
            durations: dur,
        }
    }

    #[test]
    fn transcript_text_is_trimmed_and_language_echoed() {
        let t = to_transcript(result("  hello there  ", &[], None, None), Some("ru"));
        assert_eq!(t.text, "hello there");
        assert_eq!(t.language.as_deref(), Some("ru"));
        assert!(t.tokens.is_empty());
    }

    #[test]
    fn auto_is_not_reported_as_a_detected_language() {
        let t = to_transcript(result("ni hao", &[], None, None), Some("auto"));
        assert_eq!(t.language, None);
    }

    #[test]
    fn tokens_use_durations_when_present() {
        let t = to_transcript(
            result("a b", &["a", " b"], Some(vec![0.0, 0.5]), Some(vec![0.2, 0.3])),
            None,
        );
        assert_eq!(t.tokens.len(), 2);
        assert_eq!(
            t.tokens[0],
            Token {
                text: "a".into(),
                start: 0.0,
                end: 0.2
            }
        );
        assert_eq!(
            t.tokens[1],
            Token {
                text: " b".into(),
                start: 0.5,
                end: 0.8
            }
        );
    }

    #[test]
    fn tokens_fall_back_to_the_next_start_without_durations() {
        let t = to_transcript(result("a b", &["a", " b"], Some(vec![0.0, 0.5]), None), None);
        assert_eq!(t.tokens[0].end, 0.5);
        // The last token has no successor, so it is zero-length rather than wrong.
        assert_eq!(t.tokens[1].start, 0.5);
        assert_eq!(t.tokens[1].end, 0.5);
    }

    #[test]
    fn mismatched_timestamp_counts_are_dropped_rather_than_zipped_wrong() {
        let t = to_transcript(result("a b", &["a", " b"], Some(vec![0.0]), None), None);
        assert!(t.tokens.is_empty());
    }

    #[test]
    fn an_uninitialized_engine_reports_honestly() {
        let mut engine = SherpaEngine::new(SherpaConfig::new("/nonexistent"));
        assert_eq!(engine.provider(), Provider::NativeCpu);
        assert_eq!(engine.acceleration(), Acceleration::Cpu);
        assert_eq!(engine.backend_name(), "sherpa-onnx");
        assert!(!engine.supports_streaming());
        assert!(engine.supported_languages().is_empty());
        assert!(engine.model_kind().is_none());
        assert!(engine.device().name.starts_with("CPU ("));

        let health = engine.health_check();
        assert!(!health.ok);
        assert_eq!(health.provider, Provider::NativeCpu);

        let err = engine
            .transcribe(&AudioBuffer::new(vec![0.0; 16], 16_000))
            .unwrap_err();
        assert!(err.to_string().contains("not initialized"), "{err}");
    }

    #[test]
    fn initialize_fails_cleanly_on_a_directory_with_no_model() {
        let dir = tempfile::tempdir().unwrap();
        let mut engine = SherpaEngine::new(SherpaConfig::new(dir.path()));
        let err = engine
            .initialize(&EngineInitContext {
                model_dir: dir.path().to_path_buf(),
                cache_dir: dir.path().to_path_buf(),
                cpu_threads: 2,
            })
            .unwrap_err();
        assert!(err.to_string().contains("unrecognized model layout"), "{err}");
        assert!(!engine.is_initialized());
    }
}
