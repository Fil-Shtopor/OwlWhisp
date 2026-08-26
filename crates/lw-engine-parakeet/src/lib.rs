//! # lw-engine-parakeet
//!
//! NVIDIA Parakeet TDT 0.6B v3 speech engine. Composes a mel front end ([`mel`]), a pluggable
//! encoder backend ([`encoder`], CPU or Qualcomm HTP), and a TDT greedy decoder ([`tdt`]) into a
//! [`lw_core::engine::SpeechEngine`] ([`ParakeetEngine`]).
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod error;
pub mod encoder;
pub mod engine;
pub mod mel;
pub mod merge;
pub mod tdt;
pub mod vocab;

pub use error::{Error, Result};
pub use engine::{BackendKind, ParakeetConfig, ParakeetEngine, LANGUAGES};
