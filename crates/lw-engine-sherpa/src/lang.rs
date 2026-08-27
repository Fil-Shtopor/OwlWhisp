//! Per-model-family language tables.
//!
//! [`lw_core::engine::Language`] wraps a `&'static str`, so every list here is a compile-time
//! constant. The lists describe what the *model family* supports; a specific export can be
//! narrower (an English-only Whisper build, a Chinese-only Paraformer), which is why
//! [`languages_for`] takes the detected [`ModelFiles`] rather than just the [`ModelKind`].

use lw_core::engine::Language;

use crate::detect::{ModelFiles, ModelKind};

/// English only.
pub const ENGLISH_ONLY: &[Language] = &[Language("en")];

/// The languages of multilingual Whisper: 99 through `large-v2`, plus `yue` in `large-v3`.
/// A `large-v2` model asked for `yue` will simply fail at decode time rather than silently
/// transcribe the wrong language, so the superset is the honest list to advertise.
pub const WHISPER_MULTILINGUAL: &[Language] = &[
    Language("en"),
    Language("zh"),
    Language("de"),
    Language("es"),
    Language("ru"),
    Language("ko"),
    Language("fr"),
    Language("ja"),
    Language("pt"),
    Language("tr"),
    Language("pl"),
    Language("ca"),
    Language("nl"),
    Language("ar"),
    Language("sv"),
    Language("it"),
    Language("id"),
    Language("hi"),
    Language("fi"),
    Language("vi"),
    Language("he"),
    Language("uk"),
    Language("el"),
    Language("ms"),
    Language("cs"),
    Language("ro"),
    Language("da"),
    Language("hu"),
    Language("ta"),
    Language("no"),
    Language("th"),
    Language("ur"),
    Language("hr"),
    Language("bg"),
    Language("lt"),
    Language("la"),
    Language("mi"),
    Language("ml"),
    Language("cy"),
    Language("sk"),
    Language("te"),
    Language("fa"),
    Language("lv"),
    Language("bn"),
    Language("sr"),
    Language("az"),
    Language("sl"),
    Language("kn"),
    Language("et"),
    Language("mk"),
    Language("br"),
    Language("eu"),
    Language("is"),
    Language("hy"),
    Language("ne"),
    Language("mn"),
    Language("bs"),
    Language("kk"),
    Language("sq"),
    Language("sw"),
    Language("gl"),
    Language("mr"),
    Language("pa"),
    Language("si"),
    Language("km"),
    Language("sn"),
    Language("yo"),
    Language("so"),
    Language("af"),
    Language("oc"),
    Language("ka"),
    Language("be"),
    Language("tg"),
    Language("sd"),
    Language("gu"),
    Language("am"),
    Language("yi"),
    Language("lo"),
    Language("uz"),
    Language("fo"),
    Language("ht"),
    Language("ps"),
    Language("tk"),
    Language("nn"),
    Language("mt"),
    Language("sa"),
    Language("lb"),
    Language("my"),
    Language("bo"),
    Language("tl"),
    Language("mg"),
    Language("as"),
    Language("tt"),
    Language("haw"),
    Language("ln"),
    Language("ha"),
    Language("ba"),
    Language("jw"),
    Language("su"),
    Language("yue"),
];

/// SenseVoice Small.
pub const SENSE_VOICE: &[Language] = &[
    Language("zh"),
    Language("en"),
    Language("ja"),
    Language("ko"),
    Language("yue"),
];

/// Paraformer (the upstream releases are Chinese, with some English).
pub const PARAFORMER: &[Language] = &[Language("zh"), Language("en")];

/// NVIDIA Parakeet TDT v3 / the multilingual NeMo transducer releases: 25 European languages.
///
/// A monolingual transducer export (e.g. `parakeet-tdt-0.6b-v2`, English) supports fewer; sherpa
/// does not expose the list, so this is the superset the family is trained on.
pub const NEMO_TRANSDUCER: &[Language] = &[
    Language("bg"),
    Language("hr"),
    Language("cs"),
    Language("da"),
    Language("nl"),
    Language("en"),
    Language("et"),
    Language("fi"),
    Language("fr"),
    Language("de"),
    Language("el"),
    Language("hu"),
    Language("it"),
    Language("lv"),
    Language("lt"),
    Language("mt"),
    Language("pl"),
    Language("pt"),
    Language("ro"),
    Language("sk"),
    Language("sl"),
    Language("es"),
    Language("sv"),
    Language("ru"),
    Language("uk"),
];

/// The languages the detected model can transcribe.
pub fn languages_for(files: &ModelFiles) -> &'static [Language] {
    match files {
        ModelFiles::Whisper { english_only, .. } => {
            if *english_only {
                ENGLISH_ONLY
            } else {
                WHISPER_MULTILINGUAL
            }
        }
        // Upstream Moonshine releases (tiny/base, and the v2 exports on the sherpa-onnx model
        // page) are English-only.
        ModelFiles::MoonshineV1 { .. } | ModelFiles::MoonshineV2 { .. } => ENGLISH_ONLY,
        ModelFiles::NemoTransducer { .. } => NEMO_TRANSDUCER,
        ModelFiles::SenseVoice { .. } => SENSE_VOICE,
        ModelFiles::Paraformer { .. } => PARAFORMER,
    }
}

/// The languages a *family* supports, when no concrete model has been detected yet — used by the
/// CLI/UI to describe a model before it is downloaded.
pub fn languages_for_kind(kind: ModelKind) -> &'static [Language] {
    match kind {
        ModelKind::Whisper => WHISPER_MULTILINGUAL,
        ModelKind::Moonshine => ENGLISH_ONLY,
        ModelKind::NemoTransducer => NEMO_TRANSDUCER,
        ModelKind::SenseVoice => SENSE_VOICE,
        ModelKind::Paraformer => PARAFORMER,
    }
}

/// Whether `code` (a BCP-47-ish language tag) is in `langs`, comparing case-insensitively and
/// ignoring any region subtag (`en-US` matches `en`).
pub fn supports(langs: &[Language], code: &str) -> bool {
    let base = code.split(['-', '_']).next().unwrap_or(code).to_ascii_lowercase();
    langs.iter().any(|l| l.0 == base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn whisper(english_only: bool) -> ModelFiles {
        ModelFiles::Whisper {
            encoder: PathBuf::from("e.onnx"),
            decoder: PathBuf::from("d.onnx"),
            tokens: PathBuf::from("t.txt"),
            english_only,
        }
    }

    #[test]
    fn whisper_lists_depend_on_the_export() {
        assert_eq!(languages_for(&whisper(true)), ENGLISH_ONLY);
        assert_eq!(languages_for(&whisper(false)).len(), 100);
        assert!(languages_for(&whisper(false)).contains(&Language("ru")));
    }

    #[test]
    fn no_language_list_has_duplicates_or_bad_codes() {
        for list in [
            WHISPER_MULTILINGUAL,
            SENSE_VOICE,
            PARAFORMER,
            NEMO_TRANSDUCER,
            ENGLISH_ONLY,
        ] {
            let unique: BTreeSet<&str> = list.iter().map(|l| l.0).collect();
            assert_eq!(unique.len(), list.len(), "duplicate code in {list:?}");
            for l in list {
                assert!(
                    (2..=3).contains(&l.0.len()) && l.0.chars().all(|c| c.is_ascii_lowercase()),
                    "bad language code {:?}",
                    l.0
                );
            }
        }
    }

    #[test]
    fn every_kind_has_a_non_empty_list() {
        for kind in ModelKind::ALL {
            assert!(!languages_for_kind(*kind).is_empty(), "{kind} has no languages");
        }
    }

    #[test]
    fn nemo_transducer_covers_the_fixture_languages() {
        // The repo fixtures are en/ru/es/uk; Parakeet v3 is expected to cover all four.
        for code in ["en", "ru", "es", "uk"] {
            assert!(supports(NEMO_TRANSDUCER, code), "missing {code}");
        }
    }

    #[test]
    fn supports_ignores_region_and_case() {
        assert!(supports(WHISPER_MULTILINGUAL, "en-US"));
        assert!(supports(WHISPER_MULTILINGUAL, "RU"));
        assert!(supports(SENSE_VOICE, "ja_JP"));
        assert!(!supports(ENGLISH_ONLY, "ru"));
        assert!(!supports(SENSE_VOICE, "de"));
    }
}
