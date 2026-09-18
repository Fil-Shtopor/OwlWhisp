//! Detecting which sherpa-onnx model family lives in a directory, from the file names alone.
//!
//! sherpa-onnx does not auto-discover models: the caller must fill in exactly one model-family
//! struct with exact file paths. Every upstream release archive follows a fixed naming convention,
//! so we can recover the family from a directory listing. That is what this module does — and
//! because it only looks at names, all of it is unit-testable without downloading a single model.
//!
//! # Expected layouts
//!
//! | Kind | Required files (in the model directory) |
//! |---|---|
//! | [`ModelKind::Whisper`] | `<name>-encoder.onnx`, `<name>-decoder.onnx`, `<name>-tokens.txt` (e.g. `tiny.en-encoder.onnx`) |
//! | [`ModelKind::Moonshine`] (v1) | `preprocess.onnx`, `encode.onnx`, `uncached_decode.onnx`, `cached_decode.onnx`, `tokens.txt` |
//! | [`ModelKind::Moonshine`] (v2) | `encoder.onnx`, `merged_decoder.onnx`, `tokens.txt` |
//! | [`ModelKind::NemoTransducer`] | `encoder.onnx`, `decoder.onnx`, `joiner.onnx`, `tokens.txt` (Parakeet TDT / Zipformer transducer) |
//! | [`ModelKind::SenseVoice`] | `model.onnx`, `tokens.txt`, in a directory whose name mentions `sense-voice` |
//! | [`ModelKind::Paraformer`] | `model.onnx`, `tokens.txt`, in a directory whose name mentions `paraformer` |
//! | [`ModelKind::Omnilingual`] | `model.onnx`, `tokens.txt`, in a directory whose name mentions `omnilingual` |
//!
//! Any `.onnx` file may instead (or additionally) appear as `<stem>.int8.onnx`, `.fp16.onnx`,
//! `.q8.onnx`, `.int4.onnx` or `.quant.onnx`; [`detect`] picks between the full-precision and the
//! quantized variant according to `prefer_quantized`, and falls back to whichever one exists.
//!
//! A lone `model.onnx` is genuinely ambiguous between SenseVoice and Paraformer, so the directory
//! name is used as the tie-breaker. Pass an explicit `hint` (from
//! [`SherpaConfig::model_kind`](crate::SherpaConfig::model_kind)) to settle it deterministically.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The model families this engine can drive through sherpa-onnx.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ModelKind {
    /// OpenAI Whisper (tiny … large-v3-turbo, incl. the `distil-*` exports).
    Whisper,
    /// Useful Sensors Moonshine (v1 four-file and v2 two-file exports).
    Moonshine,
    /// A NeMo/k2 offline transducer: NVIDIA Parakeet TDT, Zipformer transducer, …
    NemoTransducer,
    /// Alibaba SenseVoice Small.
    SenseVoice,
    /// Alibaba Paraformer.
    Paraformer,
    /// Alibaba Qwen3-ASR, an LLM-style encoder-decoder.
    Qwen3Asr,
    /// Meta Omnilingual ASR, a CTC model covering 1600+ languages.
    Omnilingual,
}

impl ModelKind {
    /// The stable lowercase identifier, as used by `backend_name()` and the CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            ModelKind::Whisper => "whisper",
            ModelKind::Moonshine => "moonshine",
            ModelKind::NemoTransducer => "nemo-transducer",
            ModelKind::SenseVoice => "sense-voice",
            ModelKind::Paraformer => "paraformer",
            ModelKind::Qwen3Asr => "qwen3-asr",
            ModelKind::Omnilingual => "omnilingual",
        }
    }

    /// Every kind, for CLI help text and tests.
    pub const ALL: &'static [ModelKind] = &[
        ModelKind::Whisper,
        ModelKind::Moonshine,
        ModelKind::NemoTransducer,
        ModelKind::SenseVoice,
        ModelKind::Paraformer,
        ModelKind::Qwen3Asr,
        ModelKind::Omnilingual,
    ];
}

impl std::fmt::Display for ModelKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ModelKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let key: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        match key.as_str() {
            "whisper" => Ok(ModelKind::Whisper),
            "moonshine" => Ok(ModelKind::Moonshine),
            "nemotransducer" | "transducer" | "parakeet" | "zipformer" => Ok(ModelKind::NemoTransducer),
            "sensevoice" => Ok(ModelKind::SenseVoice),
            "paraformer" => Ok(ModelKind::Paraformer),
            "qwen3asr" | "qwen3" | "qwen" => Ok(ModelKind::Qwen3Asr),
            "omnilingual" | "omniasr" => Ok(ModelKind::Omnilingual),
            _ => Err(Error::Config(format!(
                "unknown sherpa model kind {s:?}; expected one of: whisper, moonshine, \
                 nemo-transducer, sense-voice, paraformer, qwen3-asr, omnilingual"
            ))),
        }
    }
}

/// The concrete files that make up one detected model, ready to hand to sherpa-onnx.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelFiles {
    /// Whisper: separate encoder and decoder graphs plus a whisper-specific token table.
    Whisper {
        /// `<name>-encoder[.int8].onnx`.
        encoder: PathBuf,
        /// `<name>-decoder[.int8].onnx`.
        decoder: PathBuf,
        /// `<name>-tokens.txt`.
        tokens: PathBuf,
        /// Whether the export is English-only (`<name>` ends in `.en`), which forbids a language
        /// override and disables language auto-detection.
        english_only: bool,
    },
    /// Moonshine v1: four graphs.
    MoonshineV1 {
        /// `preprocess[.int8].onnx`.
        preprocessor: PathBuf,
        /// `encode[.int8].onnx`.
        encoder: PathBuf,
        /// `uncached_decode[.int8].onnx`.
        uncached_decoder: PathBuf,
        /// `cached_decode[.int8].onnx`.
        cached_decoder: PathBuf,
        /// `tokens.txt`.
        tokens: PathBuf,
    },
    /// Moonshine v2: encoder plus a single merged decoder.
    MoonshineV2 {
        /// `encoder[.int8].onnx`.
        encoder: PathBuf,
        /// `merged_decoder[.int8].onnx`.
        merged_decoder: PathBuf,
        /// `tokens.txt`.
        tokens: PathBuf,
    },
    /// An offline transducer (Parakeet TDT, Zipformer transducer, …).
    NemoTransducer {
        /// `encoder[.int8].onnx`.
        encoder: PathBuf,
        /// `decoder[.int8].onnx`.
        decoder: PathBuf,
        /// `joiner[.int8].onnx`.
        joiner: PathBuf,
        /// `tokens.txt`.
        tokens: PathBuf,
    },
    /// SenseVoice Small: one graph.
    SenseVoice {
        /// `model[.int8].onnx`.
        model: PathBuf,
        /// `tokens.txt`.
        tokens: PathBuf,
    },
    /// Paraformer: one graph.
    Paraformer {
        /// `model[.int8].onnx`.
        model: PathBuf,
        /// `tokens.txt`.
        tokens: PathBuf,
    },
    /// Meta Omnilingual ASR: one CTC graph and a token list -- the same shape as SenseVoice and
    /// Paraformer, so the three are told apart by the directory name (see `detect`).
    Omnilingual {
        /// `model[.int8].onnx`.
        model: PathBuf,
        /// `tokens.txt`.
        tokens: PathBuf,
    },
    /// Qwen3-ASR: a convolutional frontend, an encoder, a decoder, and a **tokenizer directory**
    /// rather than a token list — it is an LLM-style decoder and carries a BPE vocabulary.
    Qwen3Asr {
        /// `conv_frontend.onnx`.
        conv_frontend: PathBuf,
        /// `encoder[.int8].onnx`.
        encoder: PathBuf,
        /// `decoder[.int8].onnx`.
        decoder: PathBuf,
        /// The `tokenizer/` directory (`vocab.json`, `merges.txt`, `tokenizer_config.json`).
        tokenizer: PathBuf,
    },
}

impl ModelFiles {
    /// The family this layout belongs to.
    pub fn kind(&self) -> ModelKind {
        match self {
            ModelFiles::Whisper { .. } => ModelKind::Whisper,
            ModelFiles::MoonshineV1 { .. } | ModelFiles::MoonshineV2 { .. } => ModelKind::Moonshine,
            ModelFiles::NemoTransducer { .. } => ModelKind::NemoTransducer,
            ModelFiles::SenseVoice { .. } => ModelKind::SenseVoice,
            ModelFiles::Paraformer { .. } => ModelKind::Paraformer,
            ModelFiles::Qwen3Asr { .. } => ModelKind::Qwen3Asr,
            ModelFiles::Omnilingual { .. } => ModelKind::Omnilingual,
        }
    }

    /// Every file the layout references, for existence checks and diagnostics.
    pub fn files(&self) -> Vec<&Path> {
        match self {
            ModelFiles::Whisper {
                encoder,
                decoder,
                tokens,
                ..
            } => vec![encoder, decoder, tokens],
            ModelFiles::MoonshineV1 {
                preprocessor,
                encoder,
                uncached_decoder,
                cached_decoder,
                tokens,
            } => vec![preprocessor, encoder, uncached_decoder, cached_decoder, tokens],
            ModelFiles::MoonshineV2 {
                encoder,
                merged_decoder,
                tokens,
            } => vec![encoder, merged_decoder, tokens],
            ModelFiles::NemoTransducer {
                encoder,
                decoder,
                joiner,
                tokens,
            } => vec![encoder, decoder, joiner, tokens],
            ModelFiles::SenseVoice { model, tokens }
            | ModelFiles::Paraformer { model, tokens }
            | ModelFiles::Omnilingual { model, tokens } => {
                vec![model, tokens]
            }
            ModelFiles::Qwen3Asr {
                conv_frontend,
                encoder,
                decoder,
                tokenizer,
            } => vec![conv_frontend, encoder, decoder, tokenizer],
        }
    }

    /// Fail if any referenced path is missing from disk.
    ///
    /// "Exists", not "is a file": [`ModelFiles::Qwen3Asr`] references a tokenizer **directory**,
    /// which is how sherpa-onnx takes a BPE vocabulary. Requiring a file rejected a model that
    /// had installed perfectly, with a message naming a path that was plainly there.
    pub fn verify_exists(&self) -> Result<()> {
        for f in self.files() {
            if !f.exists() {
                return Err(Error::MissingFile(f.display().to_string()));
            }
        }
        Ok(())
    }
}

/// Quantization/precision suffixes that may sit between the stem and `.onnx`.
const PRECISION_SUFFIXES: &[&str] = &[".int8", ".fp16", ".q8", ".int4", ".quant"];

/// Split `foo.int8.onnx` into (`"foo"`, quantized = true) and `foo.onnx` into (`"foo"`, false).
/// Returns `None` for anything that is not an `.onnx` file.
fn onnx_stem(name: &str) -> Option<(&str, bool)> {
    let stem = name.strip_suffix(".onnx")?;
    for suffix in PRECISION_SUFFIXES {
        if let Some(base) = stem.strip_suffix(suffix) {
            return Some((base, true));
        }
    }
    Some((stem, false))
}

/// The full-precision and quantized file names found for one stem.
#[derive(Clone, Debug, Default)]
struct Variants {
    plain: Option<String>,
    quantized: Option<String>,
}

impl Variants {
    fn pick(&self, prefer_quantized: bool) -> Option<&str> {
        let (first, second) = if prefer_quantized {
            (&self.quantized, &self.plain)
        } else {
            (&self.plain, &self.quantized)
        };
        first.as_deref().or(second.as_deref())
    }
}

/// An indexed directory listing: ONNX stems mapped to their variants, plus the plain files.
struct Listing {
    onnx: BTreeMap<String, Variants>,
    plain: Vec<String>,
}

impl Listing {
    fn build(names: &[String]) -> Self {
        let mut onnx: BTreeMap<String, Variants> = BTreeMap::new();
        let mut plain = Vec::new();
        for name in names {
            match onnx_stem(name) {
                Some((stem, quantized)) => {
                    let entry = onnx.entry(stem.to_string()).or_default();
                    let slot = if quantized {
                        &mut entry.quantized
                    } else {
                        &mut entry.plain
                    };
                    // Keep the first match so the result is stable for a sorted listing.
                    slot.get_or_insert_with(|| name.clone());
                }
                None => plain.push(name.clone()),
            }
        }
        Self { onnx, plain }
    }

    fn has_plain(&self, name: &str) -> bool {
        self.plain.iter().any(|n| n == name)
    }
}

/// Detect the model layout of `dir` from the file `names` it contains.
///
/// `names` are plain file names (not paths); the returned paths are `dir.join(name)`. `dir` is used
/// only to build those paths and as the SenseVoice/Paraformer tie-breaker, so tests can pass a fake
/// directory and a synthetic listing. Pass `hint` to force a family when the listing is ambiguous.
///
/// See the [module docs](self) for the layouts that are recognized.
pub fn detect(
    dir: &Path,
    names: &[String],
    prefer_quantized: bool,
    hint: Option<ModelKind>,
) -> Result<ModelFiles> {
    let listing = Listing::build(names);
    let join = |name: &str| dir.join(name);

    let pick = |stem: &str| -> Option<PathBuf> {
        listing
            .onnx
            .get(stem)
            .and_then(|v| v.pick(prefer_quantized))
            .map(&join)
    };
    let pick_any = |stems: &[&str]| -> Option<PathBuf> { stems.iter().find_map(|s| pick(s)) };

    let unknown = |reason: String| {
        Err(Error::UnknownLayout {
            dir: dir.display().to_string(),
            reason,
        })
    };

    // --- Qwen3-ASR: tested first because `conv_frontend.onnx` appears in no other layout, and
    // because its `encoder`/`decoder` pair would otherwise be read as a Whisper or Moonshine
    // export. It is the only family here whose vocabulary is a directory rather than a file. ---
    if hint.is_none_or(|k| k == ModelKind::Qwen3Asr)
        && let Some(conv_frontend) = pick_any(&["conv_frontend", "conv-frontend"])
    {
        let encoder =
            pick("encoder").ok_or_else(|| Error::MissingFile(join("encoder.onnx").display().to_string()))?;
        let decoder =
            pick("decoder").ok_or_else(|| Error::MissingFile(join("decoder.onnx").display().to_string()))?;
        let tokenizer = join("tokenizer");
        if !tokenizer.is_dir() {
            return Err(Error::MissingFile(tokenizer.display().to_string()));
        }
        return Ok(ModelFiles::Qwen3Asr {
            conv_frontend,
            encoder,
            decoder,
            tokenizer,
        });
    }

    // --- Moonshine v1: four graphs. The `*cached_decode` pair is unique to Moonshine, so testing
    // it first keeps `encode`/`encoder` from being confused with the transducer layout. ---
    let moonshine_v1 = pick_any(&["uncached_decode", "uncached_decoder"])
        .zip(pick_any(&["cached_decode", "cached_decoder"]));
    if hint.is_none_or(|k| k == ModelKind::Moonshine)
        && let Some((uncached_decoder, cached_decoder)) = moonshine_v1
    {
        let preprocessor = pick_any(&["preprocess", "preprocessor"])
            .ok_or_else(|| Error::MissingFile(join("preprocess.onnx").display().to_string()))?;
        let encoder = pick_any(&["encode", "encoder"])
            .ok_or_else(|| Error::MissingFile(join("encode.onnx").display().to_string()))?;
        let tokens = require_tokens(&listing, dir, None)?;
        return Ok(ModelFiles::MoonshineV1 {
            preprocessor,
            encoder,
            uncached_decoder,
            cached_decoder,
            tokens,
        });
    }

    // --- Moonshine v2: encoder + merged decoder. ---
    if hint.is_none_or(|k| k == ModelKind::Moonshine)
        && let Some(merged_decoder) = pick("merged_decoder")
    {
        let encoder = pick_any(&["encoder", "encode"])
            .ok_or_else(|| Error::MissingFile(join("encoder.onnx").display().to_string()))?;
        let tokens = require_tokens(&listing, dir, None)?;
        return Ok(ModelFiles::MoonshineV2 {
            encoder,
            merged_decoder,
            tokens,
        });
    }

    // --- Whisper: `<name>-encoder.onnx` / `<name>-decoder.onnx` / `<name>-tokens.txt`. ---
    if hint.is_none_or(|k| k == ModelKind::Whisper) {
        let prefixes: Vec<String> = listing
            .onnx
            .keys()
            .filter_map(|stem| stem.strip_suffix("-encoder"))
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        // A directory holding two Whisper sizes is ambiguous; refuse rather than guess.
        if prefixes.len() > 1 {
            return unknown(format!(
                "several Whisper models present ({}); keep one model per directory",
                prefixes.join(", ")
            ));
        }
        if let Some(prefix) = prefixes.first() {
            let encoder = pick(&format!("{prefix}-encoder")).expect("prefix came from the index");
            let decoder = pick(&format!("{prefix}-decoder")).ok_or_else(|| {
                Error::MissingFile(join(&format!("{prefix}-decoder.onnx")).display().to_string())
            })?;
            let tokens = require_tokens(&listing, dir, Some(&format!("{prefix}-tokens.txt")))?;
            return Ok(ModelFiles::Whisper {
                encoder,
                decoder,
                tokens,
                english_only: is_english_only_whisper(prefix),
            });
        }
    }

    // --- Offline transducer: encoder + decoder + joiner. ---
    if hint.is_none_or(|k| k == ModelKind::NemoTransducer)
        && let (Some(encoder), Some(decoder), Some(joiner)) =
            (pick("encoder"), pick("decoder"), pick("joiner"))
    {
        let tokens = require_tokens(&listing, dir, None)?;
        return Ok(ModelFiles::NemoTransducer {
            encoder,
            decoder,
            joiner,
            tokens,
        });
    }

    // --- Single-graph families. `model.onnx` alone cannot tell SenseVoice from Paraformer, so use
    // the explicit hint, then the directory name. ---
    if let Some(model) = pick("model") {
        let dir_name = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let kind = match hint {
            Some(ModelKind::SenseVoice) => Some(ModelKind::SenseVoice),
            Some(ModelKind::Paraformer) => Some(ModelKind::Paraformer),
            Some(ModelKind::Omnilingual) => Some(ModelKind::Omnilingual),
            Some(_) => None,
            // Order matters only in that each test is specific enough not to catch the others.
            None if dir_name.contains("sense") => Some(ModelKind::SenseVoice),
            None if dir_name.contains("paraformer") => Some(ModelKind::Paraformer),
            None if dir_name.contains("omnilingual") || dir_name.contains("omniasr") => {
                Some(ModelKind::Omnilingual)
            }
            None => None,
        };
        let tokens = require_tokens(&listing, dir, None);
        match kind {
            Some(ModelKind::SenseVoice) => {
                return Ok(ModelFiles::SenseVoice {
                    model,
                    tokens: tokens?,
                });
            }
            Some(ModelKind::Paraformer) => {
                return Ok(ModelFiles::Paraformer {
                    model,
                    tokens: tokens?,
                });
            }
            Some(ModelKind::Omnilingual) => {
                return Ok(ModelFiles::Omnilingual {
                    model,
                    tokens: tokens?,
                });
            }
            _ => {
                return unknown(
                    "found model.onnx + tokens.txt but the directory name says neither \
                     `sense-voice` nor `paraformer`; set the model kind explicitly"
                        .to_string(),
                );
            }
        }
    }

    unknown(format!(
        "no supported layout; saw {} .onnx stem(s): [{}]. Expected Whisper \
         (<name>-encoder.onnx/<name>-decoder.onnx/<name>-tokens.txt), Moonshine \
         (preprocess/encode/uncached_decode/cached_decode + tokens.txt), a transducer \
         (encoder/decoder/joiner + tokens.txt) or SenseVoice/Paraformer (model.onnx + tokens.txt)",
        listing.onnx.len(),
        listing.onnx.keys().cloned().collect::<Vec<_>>().join(", ")
    ))
}

/// Detect the layout of a real directory on disk.
///
/// If `dir` itself holds no recognizable layout but contains exactly one subdirectory, that
/// subdirectory is tried too — extracting an upstream `.tar.bz2` leaves the model one level down.
pub fn detect_in_dir(dir: &Path, prefer_quantized: bool, hint: Option<ModelKind>) -> Result<ModelFiles> {
    let names = list_dir(dir)?;
    match detect(dir, &names, prefer_quantized, hint) {
        Ok(files) => Ok(files),
        Err(first @ Error::UnknownLayout { .. }) => {
            let mut subdirs = std::fs::read_dir(dir)
                .map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?
                .filter_map(std::result::Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir());
            match (subdirs.next(), subdirs.next()) {
                (Some(only), None) => {
                    let nested = list_dir(&only)?;
                    detect(&only, &nested, prefer_quantized, hint).map_err(|_| first)
                }
                _ => Err(first),
            }
        }
        Err(other) => Err(other),
    }
}

fn list_dir(dir: &Path) -> Result<Vec<String>> {
    if !dir.is_dir() {
        return Err(Error::Io(format!("{}: not a directory", dir.display())));
    }
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

/// Resolve the token table: an exact name when the family prescribes one, else `tokens.txt`.
fn require_tokens(listing: &Listing, dir: &Path, exact: Option<&str>) -> Result<PathBuf> {
    let wanted = exact.unwrap_or("tokens.txt");
    if listing.has_plain(wanted) {
        return Ok(dir.join(wanted));
    }
    // Some third-party exports drop the prefix on the token file; accept plain `tokens.txt` too.
    if exact.is_some() && listing.has_plain("tokens.txt") {
        return Ok(dir.join("tokens.txt"));
    }
    Err(Error::MissingFile(dir.join(wanted).display().to_string()))
}

/// Whisper exports whose name ends in `.en` are English-only: they have no language tokens, so a
/// language override is meaningless and auto-detection is unavailable.
fn is_english_only_whisper(prefix: &str) -> bool {
    let p = prefix.to_ascii_lowercase();
    p.ends_with(".en") || p.ends_with("-en") || p.ends_with("_en")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn d(name: &str) -> PathBuf {
        PathBuf::from("/models").join(name)
    }

    #[test]
    fn onnx_stem_strips_precision_suffix() {
        assert_eq!(onnx_stem("encoder.onnx"), Some(("encoder", false)));
        assert_eq!(onnx_stem("encoder.int8.onnx"), Some(("encoder", true)));
        assert_eq!(onnx_stem("model.fp16.onnx"), Some(("model", true)));
        // `.en` must not be mistaken for a precision suffix.
        assert_eq!(
            onnx_stem("tiny.en-encoder.onnx"),
            Some(("tiny.en-encoder", false))
        );
        assert_eq!(
            onnx_stem("tiny.en-encoder.int8.onnx"),
            Some(("tiny.en-encoder", true))
        );
        assert_eq!(onnx_stem("tokens.txt"), None);
    }

    #[test]
    fn detects_whisper_english_only() {
        let listing = names(&[
            "tiny.en-decoder.int8.onnx",
            "tiny.en-decoder.onnx",
            "tiny.en-encoder.int8.onnx",
            "tiny.en-encoder.onnx",
            "tiny.en-tokens.txt",
        ]);
        let files = detect(Path::new("/models"), &listing, false, None).unwrap();
        assert_eq!(files.kind(), ModelKind::Whisper);
        assert_eq!(
            files,
            ModelFiles::Whisper {
                encoder: d("tiny.en-encoder.onnx"),
                decoder: d("tiny.en-decoder.onnx"),
                tokens: d("tiny.en-tokens.txt"),
                english_only: true,
            }
        );
    }

    #[test]
    fn whisper_prefers_quantized_when_asked() {
        let listing = names(&[
            "tiny.en-decoder.int8.onnx",
            "tiny.en-decoder.onnx",
            "tiny.en-encoder.int8.onnx",
            "tiny.en-encoder.onnx",
            "tiny.en-tokens.txt",
        ]);
        let files = detect(Path::new("/models"), &listing, true, None).unwrap();
        assert_eq!(
            files,
            ModelFiles::Whisper {
                encoder: d("tiny.en-encoder.int8.onnx"),
                decoder: d("tiny.en-decoder.int8.onnx"),
                tokens: d("tiny.en-tokens.txt"),
                english_only: true,
            }
        );
    }

    #[test]
    fn falls_back_to_the_only_variant_present() {
        // Quantized-only directory, but the caller asked for full precision.
        let listing = names(&[
            "large-v3-encoder.int8.onnx",
            "large-v3-decoder.int8.onnx",
            "large-v3-tokens.txt",
        ]);
        let files = detect(Path::new("/models"), &listing, false, None).unwrap();
        match files {
            ModelFiles::Whisper {
                encoder,
                english_only,
                ..
            } => {
                assert_eq!(encoder, d("large-v3-encoder.int8.onnx"));
                assert!(!english_only, "large-v3 is multilingual");
            }
            other => panic!("expected Whisper, got {other:?}"),
        }
    }

    #[test]
    fn detects_moonshine_v1() {
        let listing = names(&[
            "cached_decode.int8.onnx",
            "encode.int8.onnx",
            "preprocess.onnx",
            "tokens.txt",
            "uncached_decode.int8.onnx",
        ]);
        let files = detect(Path::new("/models/moonshine-tiny-en"), &listing, true, None).unwrap();
        assert_eq!(files.kind(), ModelKind::Moonshine);
        match files {
            ModelFiles::MoonshineV1 {
                preprocessor,
                encoder,
                uncached_decoder,
                cached_decoder,
                ..
            } => {
                assert!(preprocessor.ends_with("preprocess.onnx"));
                assert!(encoder.ends_with("encode.int8.onnx"));
                // `cached_decode` is a suffix of `uncached_decode`: they must not be confused.
                assert!(uncached_decoder.ends_with("uncached_decode.int8.onnx"));
                assert!(cached_decoder.ends_with("cached_decode.int8.onnx"));
            }
            other => panic!("expected Moonshine v1, got {other:?}"),
        }
    }

    #[test]
    fn detects_moonshine_v2() {
        let listing = names(&["encoder.int8.onnx", "merged_decoder.int8.onnx", "tokens.txt"]);
        let files = detect(Path::new("/models/moonshine-v2"), &listing, true, None).unwrap();
        assert_eq!(files.kind(), ModelKind::Moonshine);
        assert!(matches!(files, ModelFiles::MoonshineV2 { .. }));
    }

    #[test]
    fn detects_nemo_transducer() {
        let listing = names(&[
            "decoder.int8.onnx",
            "encoder.int8.onnx",
            "joiner.int8.onnx",
            "tokens.txt",
        ]);
        let dir = Path::new("/models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8");
        let files = detect(dir, &listing, true, None).unwrap();
        assert_eq!(files.kind(), ModelKind::NemoTransducer);
        assert_eq!(files.files().len(), 4);
    }

    #[test]
    fn transducer_without_joiner_is_not_a_transducer() {
        let listing = names(&["decoder.onnx", "encoder.onnx", "tokens.txt"]);
        let err = detect(Path::new("/models/x"), &listing, false, None).unwrap_err();
        assert!(matches!(err, Error::UnknownLayout { .. }), "got {err:?}");
    }

    #[test]
    fn single_graph_disambiguated_by_directory_name() {
        let listing = names(&["model.int8.onnx", "tokens.txt"]);
        let sense = detect(
            Path::new("/models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17"),
            &listing,
            true,
            None,
        )
        .unwrap();
        assert_eq!(sense.kind(), ModelKind::SenseVoice);

        let para = detect(
            Path::new("/models/sherpa-onnx-paraformer-zh-2023-09-14"),
            &listing,
            true,
            None,
        )
        .unwrap();
        assert_eq!(para.kind(), ModelKind::Paraformer);
    }

    #[test]
    fn single_graph_without_a_hint_is_rejected() {
        let listing = names(&["model.onnx", "tokens.txt"]);
        let err = detect(Path::new("/models/mystery"), &listing, false, None).unwrap_err();
        assert!(matches!(err, Error::UnknownLayout { .. }), "got {err:?}");
    }

    #[test]
    fn explicit_hint_settles_the_ambiguity() {
        let listing = names(&["model.onnx", "tokens.txt"]);
        let files = detect(
            Path::new("/models/mystery"),
            &listing,
            false,
            Some(ModelKind::SenseVoice),
        )
        .unwrap();
        assert_eq!(files.kind(), ModelKind::SenseVoice);
    }

    #[test]
    fn hint_rejects_a_mismatched_layout() {
        let listing = names(&[
            "tiny.en-encoder.onnx",
            "tiny.en-decoder.onnx",
            "tiny.en-tokens.txt",
        ]);
        let err = detect(
            Path::new("/models/x"),
            &listing,
            false,
            Some(ModelKind::NemoTransducer),
        )
        .unwrap_err();
        assert!(matches!(err, Error::UnknownLayout { .. }), "got {err:?}");
    }

    #[test]
    fn two_whisper_models_in_one_directory_is_an_error() {
        let listing = names(&[
            "base.en-decoder.onnx",
            "base.en-encoder.onnx",
            "base.en-tokens.txt",
            "tiny.en-decoder.onnx",
            "tiny.en-encoder.onnx",
            "tiny.en-tokens.txt",
        ]);
        let err = detect(Path::new("/models/x"), &listing, false, None).unwrap_err();
        match err {
            Error::UnknownLayout { reason, .. } => {
                assert!(reason.contains("several Whisper models"), "{reason}");
            }
            other => panic!("expected UnknownLayout, got {other:?}"),
        }
    }

    #[test]
    fn missing_tokens_file_is_reported_as_missing_not_unknown() {
        let listing = names(&["tiny.en-encoder.onnx", "tiny.en-decoder.onnx"]);
        let err = detect(Path::new("/models/x"), &listing, false, None).unwrap_err();
        match err {
            Error::MissingFile(f) => assert!(f.contains("tiny.en-tokens.txt"), "{f}"),
            other => panic!("expected MissingFile, got {other:?}"),
        }
    }

    #[test]
    fn empty_directory_is_unknown() {
        let err = detect(Path::new("/models/x"), &[], false, None).unwrap_err();
        assert!(matches!(err, Error::UnknownLayout { .. }), "got {err:?}");
    }

    #[test]
    fn english_only_detection() {
        assert!(is_english_only_whisper("tiny.en"));
        assert!(is_english_only_whisper("distil-medium.en"));
        assert!(!is_english_only_whisper("large-v3"));
        assert!(!is_english_only_whisper("medium"));
    }

    #[test]
    fn model_kind_round_trips_through_strings() {
        for kind in ModelKind::ALL {
            assert_eq!(ModelKind::from_str(kind.as_str()).unwrap(), *kind);
        }
        assert_eq!(
            ModelKind::from_str("Parakeet").unwrap(),
            ModelKind::NemoTransducer
        );
        assert_eq!(ModelKind::from_str("sense_voice").unwrap(), ModelKind::SenseVoice);
        assert!(ModelKind::from_str("banana").is_err());
    }

    #[test]
    fn detect_in_dir_descends_into_a_lone_subdirectory() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("sherpa-onnx-whisper-tiny.en");
        std::fs::create_dir_all(&nested).unwrap();
        for f in [
            "tiny.en-encoder.onnx",
            "tiny.en-decoder.onnx",
            "tiny.en-tokens.txt",
        ] {
            std::fs::write(nested.join(f), b"x").unwrap();
        }
        let files = detect_in_dir(root.path(), false, None).unwrap();
        assert_eq!(files.kind(), ModelKind::Whisper);
        files.verify_exists().unwrap();
    }

    /// Qwen3-ASR is detected by `conv_frontend.onnx`, which no other family here has.
    ///
    /// Tested because its `encoder`/`decoder` pair is exactly what a Whisper or Moonshine export
    /// looks like: without the frontend being checked first, a Qwen3 directory would be handed to
    /// the wrong recognizer and fail with a confusing message about a missing token file.
    #[test]
    fn a_qwen3_layout_is_not_mistaken_for_whisper_or_moonshine() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("tokenizer")).unwrap();
        std::fs::write(dir.path().join("tokenizer/vocab.json"), b"{}").unwrap();

        let files = detect_in_dir(dir.path(), true, None).unwrap();
        assert_eq!(files.kind(), ModelKind::Qwen3Asr);
        match &files {
            ModelFiles::Qwen3Asr {
                encoder, tokenizer, ..
            } => {
                assert!(
                    encoder.ends_with("encoder.int8.onnx"),
                    "the int8 graph is preferred"
                );
                assert!(tokenizer.is_dir(), "the tokenizer is a directory, not a file");
            }
            other => panic!("expected Qwen3Asr, got {other:?}"),
        }
        // The directory must satisfy the existence check, which is what a real install failed.
        files.verify_exists().unwrap();
    }

    /// A Qwen3 directory without its tokenizer says which path is missing.
    #[test]
    fn a_qwen3_layout_without_a_tokenizer_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["conv_frontend.onnx", "encoder.onnx", "decoder.onnx"] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        match detect_in_dir(dir.path(), true, None) {
            Err(Error::MissingFile(f)) => assert!(f.ends_with("tokenizer"), "{f}"),
            other => panic!("expected a missing tokenizer, got {other:?}"),
        }
    }

    #[test]
    fn verify_exists_reports_the_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tiny.en-encoder.onnx"), b"x").unwrap();
        std::fs::write(dir.path().join("tiny.en-tokens.txt"), b"x").unwrap();
        let files = ModelFiles::Whisper {
            encoder: dir.path().join("tiny.en-encoder.onnx"),
            decoder: dir.path().join("tiny.en-decoder.onnx"),
            tokens: dir.path().join("tiny.en-tokens.txt"),
            english_only: true,
        };
        match files.verify_exists().unwrap_err() {
            Error::MissingFile(f) => assert!(f.contains("tiny.en-decoder.onnx"), "{f}"),
            other => panic!("expected MissingFile, got {other:?}"),
        }
    }
}
