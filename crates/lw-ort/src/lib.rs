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

mod manifest;
mod qnn;
mod runtime;
mod session;

pub use manifest::{RuntimeFile, RuntimeManifest};
pub use qnn::{build_qnn_session, HtpPerformanceMode, QnnSessionConfig};
pub use runtime::{locate_runtime_dir, OrtRuntime, RuntimeError};
pub use session::{build_cpu_session, CpuSessionConfig};

/// Re-export of the underlying `ort` crate.
pub use ort;
