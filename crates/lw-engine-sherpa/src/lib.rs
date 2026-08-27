//! # lw-engine-sherpa
//!
//! A **portable CPU** speech engine for LocalWisper, built on the official
//! [`sherpa-onnx`](https://crates.io/crates/sherpa-onnx) Rust bindings (k2-fsa, Apache-2.0).
//!
//! Where [`lw_engine_parakeet`] runs one model through our own ONNX Runtime layer — and reaches
//! the Qualcomm NPU by doing so — this crate trades acceleration for *choice and reach*: it runs
//! Whisper, Moonshine, SenseVoice, Paraformer and NeMo/Zipformer transducers on any x86-64 or
//! aarch64 CPU (Snapdragon, Intel, AMD, Apple), with the decoder supplied by sherpa-onnx rather
//! than written here.
//!
//! ```text
//! model directory ──▶ detect::detect_in_dir ──▶ ModelFiles ──▶ sherpa OfflineRecognizer
//!                     (file names only)          (+ SherpaConfig)      │
//!                                                                      ▼
//!                                            AudioBuffer (16 kHz mono f32) ──▶ Transcript
//! ```
//!
//! # The `sherpa` cargo feature
//!
//! The native dependency pulls a ~140 MB prebuilt archive at build time, so it is **opt-in**:
//!
//! ```toml
//! lw-engine-sherpa = { workspace = true, features = ["sherpa"] }
//! ```
//!
//! Without the feature the crate still compiles and still exports [`detect`], [`lang`] and
//! [`SherpaConfig`] — everything that reasons about models without loading them — but
//! [`SherpaEngine`] is absent. Code that must build either way can call
//! [`is_available`] and fall back.
//!
//! # Two ONNX Runtimes in one process
//!
//! `lw-ort` loads Microsoft's `onnxruntime.dll` dynamically, by full path. sherpa-onnx ships its
//! own ONNX Runtime, which the `static` feature (the default, and what the workspace pins) links
//! *into the binary* instead of placing a second `onnxruntime.dll` beside ours. The two never see
//! each other's symbols: Windows resolves our copy through an explicit `LoadLibrary` handle, and
//! sherpa's copy is resolved at link time. `tests/dual_runtime.rs` proves this holds in practice.
//!
//! # Supported model layouts
//!
//! See [`detect`] for the exact file names each family expects. In short:
//! Whisper (`<name>-encoder.onnx`), Moonshine (`preprocess`/`encode`/`*cached_decode`),
//! transducers (`encoder`/`decoder`/`joiner`), SenseVoice and Paraformer (`model.onnx`).
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]
// The error enum carries a few path-sized variants; boxing every Result is not worth it here.
#![allow(clippy::result_large_err)]

mod config;
pub mod detect;
mod error;
pub mod lang;

#[cfg(feature = "sherpa")]
mod engine;

pub use config::{DECODING_METHODS, MAX_THREADS, SherpaConfig};
pub use detect::{ModelFiles, ModelKind, detect, detect_in_dir};
pub use error::{Error, Result};
pub use lang::{languages_for, languages_for_kind};

#[cfg(feature = "sherpa")]
pub use engine::SherpaEngine;

/// Whether this build can actually load models — i.e. whether the `sherpa` feature is on.
///
/// Callers that are compiled both ways (the CLI, the Tauri app) should branch on this rather than
/// on `cfg!`, so the "engine not compiled in" message is written in one place.
pub const fn is_available() -> bool {
    cfg!(feature = "sherpa")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_matches_the_feature() {
        assert_eq!(is_available(), cfg!(feature = "sherpa"));
    }

    #[test]
    fn detection_works_without_the_native_library() {
        // The point of splitting `detect` out: model bookkeeping needs no 140 MB download.
        let names = [
            "tiny.en-encoder.onnx",
            "tiny.en-decoder.onnx",
            "tiny.en-tokens.txt",
        ]
        .map(str::to_string);
        let files = detect(std::path::Path::new("/m"), &names, false, None).unwrap();
        assert_eq!(files.kind(), ModelKind::Whisper);
        assert_eq!(languages_for(&files).len(), 1);
    }
}
