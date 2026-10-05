//! # lw-engine-parakeet
//!
//! NVIDIA Parakeet TDT 0.6B v3 speech engine. Composes a mel front end ([`mel`]), a pluggable
//! encoder backend ([`encoder`], CPU or Qualcomm HTP), and a TDT greedy decoder ([`tdt`]) into a
//! [`lw_core::engine::SpeechEngine`] ([`ParakeetEngine`]).
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]
// The ORT/engine error enums carry a few large variants (paths + messages); boxing every Result
// is not worth it for this app, so we accept the size here.
#![allow(clippy::result_large_err)]

pub mod encoder;
pub mod engine;
mod error;
pub mod mel;
pub mod merge;
pub mod npu;
pub mod tdt;
pub mod vocab;

pub use engine::{BackendKind, LANGUAGES, ParakeetConfig, ParakeetEngine};
pub use error::{Error, Result};
