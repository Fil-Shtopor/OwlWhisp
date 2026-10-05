//! # lw-ort
//!
//! The ONNX Runtime layer for OwlWhisp. It:
//! - dynamically loads a stock `onnxruntime.dll`/`.so`/`.dylib` (via `ort` `load-dynamic`),
//! - registers plugin execution providers (Qualcomm QNN, WebGPU, DirectML,
//!   OpenVINO, Vitis AI, CoreML) and handles the matched legacy CUDA GPU runtime,
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
pub mod nvidia;
mod qnn;
mod runtime;
mod session;

pub use manifest::{RuntimeFile, RuntimeManifest};
pub use qnn::{HtpPerformanceMode, QnnSessionConfig, build_qnn_session};
pub use runtime::{AcceleratorStatus, OrtRuntime, RuntimeError, locate_runtime_dir, onnxruntime_lib_name};
pub use session::{
    CpuSessionConfig, TensorRtSessionConfig, TensorRtShapeProfile, build_accel_session,
    build_accel_session_with_tensorrt_config, build_cpu_session, prepare_static_model,
};

/// Re-export of the underlying `ort` crate.
pub use ort;

/// End the process without running C `atexit` handlers.
///
/// ONNX Runtime's **WebGPU** execution provider registers teardown that crashes the process on
/// exit once a WebGPU session has existed: the work completes, the results print, and then the
/// process dies with `STATUS_STACK_BUFFER_OVERRUN` (0xC0000409). Reproduced on Windows ARM64 with
/// the WebGPU plugin EP 0.3.0 — an early release — and only ever *after* all useful work is done.
///
/// Neither `std::process::exit` nor `ExitProcess` avoids it — both were tried, and both still
/// crashed, which places the fault in `DLL_PROCESS_DETACH` rather than in a C `atexit` handler.
/// So this asks the OS to end the process outright.
///
/// **The cost is real and bounded:** nothing buffered is written after this point. Callers must
/// flush anything they care about first — this flushes the standard streams, but a caller with a
/// non-blocking log writer has to drop its guard beforehand. In exchange, a user who chose the
/// GPU does not get a crash dialog every time they quit.
///
/// **Do not** reach for this as a general shutdown. It is a workaround for one upstream bug in an
/// early (0.3.0) execution provider, and it should be deleted when that bug is fixed.
pub fn exit_without_teardown(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    #[cfg(windows)]
    // SAFETY: both calls are infallible for the current process and take no pointers we own;
    // TerminateProcess on self never returns. The streams are flushed above.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), code as u32);
        // TerminateProcess is asynchronous in principle; park rather than fall through.
        loop {
            std::thread::park();
        }
    }

    #[cfg(not(windows))]
    {
        // No equivalent problem is known off Windows, so take the ordinary path there.
        std::process::exit(code)
    }
}
