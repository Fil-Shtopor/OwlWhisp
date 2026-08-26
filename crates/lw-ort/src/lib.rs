//! # lw-ort
//!
//! The ONNX Runtime layer for LocalWisper. It:
//! - dynamically loads a stock `onnxruntime.dll`/`.so`/`.dylib` (via `ort` `load-dynamic`),
//! - registers Qualcomm's plugin QNN execution provider and enumerates the NPU device,
//! - builds CPU and QNN/HTP sessions with the right options and EPContext caching,
//! - verifies a pinned runtime manifest (per-file SHA-256) before use.
//!
//! It deliberately re-exports [`ort`] so the engine crates can create tensors and read outputs with
//! the upstream API while the hard, fragile parts (dynamic load, EP registration, device selection,
//! context-binary caching) live here in one place.
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]
// The ORT/engine error enums carry a few large variants (paths + messages); boxing every Result
// is not worth it for this app, so we accept the size here.
#![allow(clippy::result_large_err)]

mod manifest;
mod qnn;
mod runtime;
mod session;

pub use manifest::{RuntimeFile, RuntimeManifest};
pub use qnn::{HtpPerformanceMode, QnnSessionConfig, build_qnn_session};
pub use runtime::{OrtRuntime, RuntimeError, locate_runtime_dir, onnxruntime_lib_name};
pub use session::{CpuSessionConfig, build_cpu_session};

/// Re-export of the underlying `ort` crate.
pub use ort;
