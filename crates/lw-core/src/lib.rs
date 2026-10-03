//! # lw-core
//!
//! The hardware- and UI-agnostic core of OwlWhisp: audio primitives, a model-agnostic VAD state
//! machine, the [`SpeechEngine`](engine::SpeechEngine) abstraction and backend-selection policy, a
//! deterministic text pipeline, a replacement dictionary, per-application profiles, typed settings,
//! a hash-verified model manager, hardware-capability data, and diagnostics.
//!
//! This crate depends on nothing OS-specific. Concrete engines live in `lw-engine-*`, the ONNX
//! layer in `lw-ort`, and OS integration in `lw-platform`.
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod audio;
pub mod bench;
pub mod capabilities;
pub mod diagnostics;
pub mod dictionary;
pub mod engine;
pub mod error;
pub mod languages;
pub mod model;
pub mod profiles;
pub mod settings;
pub mod sound;
pub mod text;
pub mod vad;

pub use error::{Error, Result};

/// The crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
