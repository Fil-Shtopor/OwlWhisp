//! What this machine is, probed once.
//!
//! Lives here rather than in a command layer because "is the NPU usable" is a fact about the
//! machine, not about a window, and every front end needs the same answer.

use std::sync::OnceLock;

use lw_core::capabilities::Capabilities;

/// Enumerating devices is the only honest way to answer "is the NPU usable", so that is what this
/// does. It registers the QNN plugin EP process-wide, which is safe here because every CPU session
/// is pinned to the CPU device (see `lw_ort::build_cpu_session`). The result is cached: the probe
/// costs a DLL load, and the answer cannot change while the process runs.
pub fn probe_capabilities() -> &'static Capabilities {
    static CAPS: OnceLock<Capabilities> = OnceLock::new();
    CAPS.get_or_init(|| {
        let mut caps = lw_platform::caps::detect();
        caps.providers.cpu = true;
        match lw_ort::OrtRuntime::auto() {
            Ok(rt) => caps.providers.qnn = rt.has_qnn_npu(),
            Err(e) => tracing::warn!(
                "ONNX Runtime not loaded ({e}); accelerator availability is unverified,                  so everything reported here assumes CPU"
            ),
        }
        caps
    })
}

