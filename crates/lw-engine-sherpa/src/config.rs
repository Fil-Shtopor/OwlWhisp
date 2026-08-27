//! Configuration for [`SherpaEngine`](crate::SherpaEngine).

use std::path::{Path, PathBuf};

use lw_core::engine::EngineInitContext;
use serde::{Deserialize, Serialize};

use crate::detect::ModelKind;
use crate::error::{Error, Result};
use crate::lang;

/// Decoding strategies sherpa-onnx accepts for offline recognizers.
pub const DECODING_METHODS: &[&str] = &["greedy_search", "modified_beam_search"];

/// Upper bound on the thread count we will hand to sherpa-onnx. Beyond a handful of threads ONNX
/// Runtime's intra-op pool stops helping on these models and starts fighting the audio thread.
pub const MAX_THREADS: usize = 32;

/// Default thread count when the caller does not pin one: half the machine, clamped to `[1, 8]`.
fn default_threads() -> usize {
    let cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4);
    (cores / 2).clamp(1, 8)
}

/// How to build a sherpa-onnx offline recognizer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SherpaConfig {
    /// Directory holding the extracted model. See [`crate::detect`] for the expected file names.
    pub model_dir: PathBuf,
    /// Intra-op thread count for ONNX Runtime. `0` means "pick a default".
    pub num_threads: usize,
    /// Force a model family instead of auto-detecting it. Required for a bare `model.onnx` whose
    /// directory name does not say `sense-voice` or `paraformer`.
    pub model_kind: Option<ModelKind>,
    /// Language hint. Only meaningful for multilingual Whisper (where it disables auto-detection)
    /// and SenseVoice (where `"auto"` is also accepted). Ignored by the other families.
    pub language: Option<String>,
    /// `greedy_search` (default) or `modified_beam_search`.
    pub decoding_method: Option<String>,
    /// Override sherpa-onnx's own `model_type` metadata probe. Normally `None`: sherpa reads the
    /// type out of the encoder's ONNX metadata. Set it (e.g. `"nemo_transducer"`) for a
    /// third-party export whose metadata is missing or wrong.
    pub model_type: Option<String>,
    /// Beam width for `modified_beam_search`.
    pub max_active_paths: i32,
    /// Prefer `*.int8.onnx` over full precision when a directory ships both. Quantized models are
    /// several times faster on CPU at a small accuracy cost, which is the right default for an
    /// engine whose job is to run acceptably on any laptop.
    pub prefer_quantized: bool,
    /// Ask Whisper for per-token timestamps. Off by default: it costs an extra alignment pass and
    /// some third-party Whisper exports lack the metadata it needs. Transducer and Moonshine
    /// models emit timestamps unconditionally, so this flag does not affect them.
    pub whisper_token_timestamps: bool,
    /// Apply SenseVoice inverse text normalization (digits, punctuation).
    pub use_itn: bool,
    /// Turn on sherpa-onnx's own logging to stderr.
    pub debug: bool,
}

impl Default for SherpaConfig {
    fn default() -> Self {
        Self {
            model_dir: PathBuf::new(),
            num_threads: 0,
            model_kind: None,
            language: None,
            decoding_method: None,
            model_type: None,
            max_active_paths: 4,
            prefer_quantized: true,
            whisper_token_timestamps: false,
            use_itn: true,
            debug: false,
        }
    }
}

impl SherpaConfig {
    /// A config for `model_dir` with everything else defaulted.
    pub fn new(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model_dir: model_dir.into(),
            ..Self::default()
        }
    }

    /// Derive a config from the context the engine registry passes to `initialize`.
    pub fn from_ctx(ctx: &EngineInitContext) -> Self {
        Self {
            model_dir: ctx.model_dir.clone(),
            num_threads: ctx.cpu_threads,
            ..Self::default()
        }
    }

    /// Set the language hint (builder style).
    #[must_use]
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Set the thread count (builder style). `0` restores the default.
    #[must_use]
    pub fn with_threads(mut self, num_threads: usize) -> Self {
        self.num_threads = num_threads;
        self
    }

    /// Force a model family (builder style).
    #[must_use]
    pub fn with_model_kind(mut self, kind: ModelKind) -> Self {
        self.model_kind = Some(kind);
        self
    }

    /// The thread count actually used: [`Self::num_threads`] when pinned, else a machine-derived
    /// default. Always at least 1 and never above [`MAX_THREADS`].
    pub fn effective_threads(&self) -> usize {
        if self.num_threads == 0 {
            default_threads()
        } else {
            self.num_threads.min(MAX_THREADS)
        }
    }

    /// The decoding method actually used.
    pub fn effective_decoding_method(&self) -> &str {
        self.decoding_method.as_deref().unwrap_or("greedy_search")
    }

    /// Reject configurations sherpa-onnx would only fail on later (or, worse, ignore silently).
    ///
    /// This does **not** touch the filesystem beyond checking that `model_dir` is set; model files
    /// are validated during `initialize`.
    pub fn validate(&self) -> Result<()> {
        if self.model_dir.as_os_str().is_empty() {
            return Err(Error::Config("model_dir is empty".into()));
        }
        if let Some(method) = &self.decoding_method
            && !DECODING_METHODS.contains(&method.as_str())
        {
            return Err(Error::Config(format!(
                "decoding_method {method:?} is not one of {DECODING_METHODS:?}"
            )));
        }
        if self.effective_decoding_method() == "modified_beam_search" && self.max_active_paths < 1 {
            return Err(Error::Config(format!(
                "max_active_paths must be >= 1 for modified_beam_search, got {}",
                self.max_active_paths
            )));
        }
        if self.num_threads > MAX_THREADS {
            return Err(Error::Config(format!(
                "num_threads {} exceeds the {MAX_THREADS} thread cap",
                self.num_threads
            )));
        }
        if let Some(code) = &self.language {
            let looks_like_a_tag = !code.is_empty()
                && code.len() <= 8
                && code
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !looks_like_a_tag {
                return Err(Error::Config(format!(
                    "language {code:?} is not a BCP-47-like tag (e.g. \"en\", \"ru\", \"auto\")"
                )));
            }
        }
        Ok(())
    }

    /// Check the language hint against a detected model, once its family is known, and return the
    /// value that will actually be handed to sherpa-onnx.
    ///
    /// `Ok(None)` means the model takes no language argument — either because the family has none
    /// (Moonshine, transducers, Paraformer) or because it auto-detects (multilingual Whisper with
    /// no hint, English-only Whisper). An `Err` means the hint and the model contradict each other,
    /// e.g. `"ru"` against a `tiny.en` export.
    ///
    /// Callers can use this to validate a `--language` flag against a model directory *before*
    /// paying to load the model: pair it with [`detect_in_dir`](crate::detect_in_dir).
    pub fn language_for(&self, files: &crate::detect::ModelFiles) -> Result<Option<String>> {
        use crate::detect::ModelFiles;
        let Some(code) = self.language.as_deref() else {
            // SenseVoice's own default is "auto"; everything else auto-detects or is monolingual.
            return Ok(match files {
                ModelFiles::SenseVoice { .. } => Some("auto".to_string()),
                _ => None,
            });
        };
        match files {
            ModelFiles::Whisper {
                english_only: true, ..
            } => {
                if lang::supports(lang::ENGLISH_ONLY, code) {
                    Ok(None) // The `.en` exports carry no language tokens at all.
                } else {
                    Err(Error::Config(format!(
                        "this Whisper export is English-only, but language {code:?} was requested"
                    )))
                }
            }
            ModelFiles::Whisper { .. } => {
                let base = base_tag(code);
                if lang::supports(lang::WHISPER_MULTILINGUAL, &base) {
                    Ok(Some(base))
                } else {
                    Err(Error::Config(format!(
                        "Whisper does not support language {code:?}"
                    )))
                }
            }
            ModelFiles::SenseVoice { .. } => {
                if code.eq_ignore_ascii_case("auto") {
                    return Ok(Some("auto".to_string()));
                }
                let base = base_tag(code);
                if lang::supports(lang::SENSE_VOICE, &base) {
                    Ok(Some(base))
                } else {
                    Err(Error::Config(format!(
                        "SenseVoice does not support language {code:?}; it handles {:?} or \"auto\"",
                        lang::SENSE_VOICE.iter().map(|l| l.0).collect::<Vec<_>>()
                    )))
                }
            }
            // Moonshine, transducers and Paraformer take no language argument; the hint is
            // informational, so accept it but do not pass it on.
            _ => Ok(None),
        }
    }

    /// Where the model lives, for error messages.
    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }
}

fn base_tag(code: &str) -> String {
    code.split(['-', '_']).next().unwrap_or(code).to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::ModelFiles;

    fn whisper(english_only: bool) -> ModelFiles {
        ModelFiles::Whisper {
            encoder: PathBuf::from("e.onnx"),
            decoder: PathBuf::from("d.onnx"),
            tokens: PathBuf::from("t.txt"),
            english_only,
        }
    }

    fn sense_voice() -> ModelFiles {
        ModelFiles::SenseVoice {
            model: PathBuf::from("model.onnx"),
            tokens: PathBuf::from("tokens.txt"),
        }
    }

    #[test]
    fn default_is_invalid_until_a_model_dir_is_set() {
        assert!(SherpaConfig::default().validate().is_err());
        assert!(SherpaConfig::new("/models/whisper").validate().is_ok());
    }

    #[test]
    fn from_ctx_carries_dir_and_threads() {
        let ctx = EngineInitContext {
            model_dir: PathBuf::from("/models/x"),
            cache_dir: PathBuf::from("/cache"),
            cpu_threads: 6,
        };
        let cfg = SherpaConfig::from_ctx(&ctx);
        assert_eq!(cfg.model_dir, PathBuf::from("/models/x"));
        assert_eq!(cfg.effective_threads(), 6);
    }

    #[test]
    fn thread_defaults_and_caps() {
        let cfg = SherpaConfig::new("/m");
        assert!((1..=8).contains(&cfg.effective_threads()));
        assert_eq!(cfg.clone().with_threads(3).effective_threads(), 3);
        // Over the cap is rejected by validate, and clamped if it somehow gets through.
        let mut over = SherpaConfig::new("/m");
        over.num_threads = MAX_THREADS + 1;
        assert!(over.validate().is_err());
        assert_eq!(over.effective_threads(), MAX_THREADS);
    }

    #[test]
    fn decoding_method_is_checked() {
        let mut cfg = SherpaConfig::new("/m");
        assert_eq!(cfg.effective_decoding_method(), "greedy_search");
        cfg.decoding_method = Some("modified_beam_search".into());
        assert!(cfg.validate().is_ok());
        cfg.max_active_paths = 0;
        assert!(cfg.validate().is_err());
        cfg.decoding_method = Some("beam_search".into());
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn language_tags_are_checked() {
        let mut cfg = SherpaConfig::new("/m");
        cfg.language = Some("en-US".into());
        assert!(cfg.validate().is_ok());
        cfg.language = Some("Русский".into());
        assert!(cfg.validate().is_err());
        cfg.language = Some(String::new());
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn english_only_whisper_rejects_another_language() {
        let mut cfg = SherpaConfig::new("/m");
        cfg.language = Some("ru".into());
        assert!(cfg.language_for(&whisper(true)).is_err());
        cfg.language = Some("en".into());
        assert_eq!(cfg.language_for(&whisper(true)).unwrap(), None);
    }

    #[test]
    fn multilingual_whisper_normalizes_the_tag() {
        let mut cfg = SherpaConfig::new("/m");
        cfg.language = Some("RU-ru".into());
        assert_eq!(cfg.language_for(&whisper(false)).unwrap().as_deref(), Some("ru"));
        cfg.language = Some("klingon".into());
        assert!(cfg.language_for(&whisper(false)).is_err());
    }

    #[test]
    fn whisper_without_a_hint_auto_detects() {
        let cfg = SherpaConfig::new("/m");
        assert_eq!(cfg.language_for(&whisper(false)).unwrap(), None);
    }

    #[test]
    fn sense_voice_defaults_to_auto_and_accepts_its_languages() {
        let cfg = SherpaConfig::new("/m");
        assert_eq!(cfg.language_for(&sense_voice()).unwrap().as_deref(), Some("auto"));
        let cfg = SherpaConfig::new("/m").with_language("ja");
        assert_eq!(cfg.language_for(&sense_voice()).unwrap().as_deref(), Some("ja"));
        let cfg = SherpaConfig::new("/m").with_language("de");
        assert!(cfg.language_for(&sense_voice()).is_err());
    }

    #[test]
    fn families_without_a_language_argument_ignore_the_hint() {
        let files = ModelFiles::NemoTransducer {
            encoder: PathBuf::from("e"),
            decoder: PathBuf::from("d"),
            joiner: PathBuf::from("j"),
            tokens: PathBuf::from("t"),
        };
        let cfg = SherpaConfig::new("/m").with_language("uk");
        assert_eq!(cfg.language_for(&files).unwrap(), None);
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = SherpaConfig::new("/models/whisper-tiny.en")
            .with_language("en")
            .with_threads(4)
            .with_model_kind(ModelKind::Whisper);
        let json = serde_json::to_string(&cfg).unwrap();
        let back: SherpaConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
