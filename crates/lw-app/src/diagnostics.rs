//! What this build is, what runtime it found, and what the hardware will actually let it use.
//!
//! The Diagnostics tab exists to answer one question honestly -- "is the NPU being used, and if
//! not, why not" -- so every field here is something observed, never something assumed. In
//! particular `present`, `registered`, `devices` and `usable` are kept apart because they routinely
//! disagree: a driver package can be installed while the execution provider fails to load, and
//! only `usable` means acceleration.

use serde::Serialize;

/// One accelerator as this machine actually reports it.
#[derive(Clone, Debug, Serialize)]
pub struct AcceleratorStatus {
    pub id: &'static str,
    pub label: &'static str,
    pub kind_label: &'static str,
    pub vendor: Option<&'static str>,
    pub library: Option<&'static str>,
    /// True when this accelerator needs a model graph compiled for it. NPUs do; GPUs do not.
    pub needs_dedicated_artifact: bool,
    /// The provider library is on disk.
    pub present: bool,
    /// The provider registered with ONNX Runtime.
    pub registered: bool,
    /// How many devices it enumerated. Registering without enumerating one is the common failure.
    pub devices: usize,
    /// The only one of these that means acceleration.
    pub usable: bool,
    /// A sentence saying which of the above failed, for a reader who is not going to cross-
    /// reference four booleans.
    pub detail: String,
}

/// Everything the Diagnostics tab shows.
#[derive(Clone, Debug, Serialize)]
pub struct Diagnostics {
    pub app_version: &'static str,
    pub core_version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    /// Where the ONNX Runtime was loaded from, when it loaded at all.
    pub runtime_dir: Option<String>,
    /// Why it did not, when it did not. Present and `runtime_dir` absent means the same thing said
    /// usefully rather than as a missing value.
    pub runtime_error: Option<String>,
    pub qnn_dll_present: bool,
    pub qnn_registered: bool,
    pub qnn_npu_count: usize,
    pub npu_available: bool,
    pub devices: Vec<String>,
    pub accelerators: Vec<AcceleratorStatus>,
    /// Set when the accelerator table could not be built at all, which is not the same as every
    /// accelerator being unavailable -- saying so avoids reporting ignorance as a hardware verdict.
    pub accelerators_error: Option<String>,
}

/// Probe the machine. Loads `onnxruntime`, so a caller should not run it on a UI thread.
pub fn collect(app_version: &'static str) -> Diagnostics {
    let mut d = Diagnostics {
        app_version,
        core_version: lw_core::VERSION,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        runtime_dir: None,
        runtime_error: None,
        qnn_dll_present: false,
        qnn_registered: false,
        qnn_npu_count: 0,
        npu_available: false,
        devices: Vec::new(),
        accelerators: Vec::new(),
        accelerators_error: None,
    };

    match lw_ort::OrtRuntime::auto() {
        Ok(rt) => {
            d.runtime_dir = Some(rt.runtime_dir().display().to_string());
            d.qnn_dll_present = rt.qnn_available();
            // Probing registers the QNN plugin EP process-wide. That used to mean probe order
            // mattered -- a registered EP gets auto-applied to later sessions. It no longer does:
            // every CPU session is pinned to the CPU *device*, so a registered provider cannot be
            // applied where it was not asked for (see lw-ort's session builder).
            d.npu_available = rt.has_qnn_npu();
            d.qnn_registered = rt.qnn_registered();
            d.qnn_npu_count = rt.qnn_npu_count();
            d.devices = rt.device_summary();
            d.accelerators = rt
                .probe_accelerators()
                .into_iter()
                .map(|st| AcceleratorStatus {
                    id: st.accel.id(),
                    label: st.accel.label(),
                    kind_label: st.accel.kind().label(),
                    vendor: st.accel.vendor(),
                    library: st.accel.library_file(),
                    needs_dedicated_artifact: st.accel.needs_dedicated_artifact(),
                    present: st.present,
                    registered: st.registered,
                    usable: st.usable(),
                    detail: st.explain(),
                    devices: st.devices,
                })
                .collect();
        }
        Err(e) => {
            d.runtime_error = Some(e.to_string());
            d.accelerators_error =
                Some("ONNX Runtime not found; accelerator availability is unknown".into());
        }
    }
    d
}
