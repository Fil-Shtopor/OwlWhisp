//! What this build is, what runtime it found, and what the hardware will actually let it use.
//!
//! The Diagnostics tab exists to answer one question honestly -- "is the NPU being used, and if
//! not, why not" -- so every field here is something observed, never something assumed. In
//! particular `present`, `registered`, `devices` and `usable` are kept apart because they routinely
//! disagree: a driver package can be installed while the execution provider fails to load, and
//! only `usable` means acceleration.

use serde::Serialize;

/// Merge independent OS, provider and NVIDIA driver observations into hardware descriptions.
/// The driver API also reports headless/TCC NVIDIA devices absent from the display-adapter list.
pub fn hardware_device_descriptions(
    provider_devices: &[String],
    physical_devices: &[String],
    nvidia: &[lw_ort::nvidia::NvidiaDevice],
) -> Vec<String> {
    let mut devices = provider_devices.to_vec();
    devices.extend_from_slice(physical_devices);
    devices.extend(
        nvidia
            .iter()
            .map(|gpu| format!("NVIDIA driver: {} GPU (ordinal {})", gpu.name, gpu.ordinal)),
    );
    devices
}

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
    /// Whether this computer has the target hardware, independent of runtime installation.
    pub hardware_present: bool,
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
    /// An actionable package or driver installation for this physical device.
    pub setup_action: Option<crate::runtime_install::SetupAction>,
}

/// Everything the Diagnostics tab shows.
#[derive(Clone, Debug, Serialize)]
pub struct Diagnostics {
    pub app_version: &'static str,
    pub core_version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    /// Whether the portable sherpa engine was compiled into this application.
    pub sherpa_enabled: bool,
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
        sherpa_enabled: cfg!(feature = "sherpa"),
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
            let machine = lw_platform::caps::detect();
            let has_qualcomm_soc =
                lw_core::capabilities::Accelerator::QnnNpu.relevant_to_hardware(&machine.cpu_brand, &[]);
            // Probing registers the QNN plugin EP process-wide. That used to mean probe order
            // mattered -- a registered EP gets auto-applied to later sessions. It no longer does:
            // every CPU session is pinned to the CPU *device*, so a registered provider cannot be
            // applied where it was not asked for (see lw-ort's session builder).
            if has_qualcomm_soc {
                d.qnn_dll_present = rt.qnn_available();
                d.npu_available = rt.has_qnn_npu();
                d.qnn_registered = rt.qnn_registered();
                d.qnn_npu_count = rt.qnn_npu_count();
            }
            let mut probes = lw_core::capabilities::ALL_ACCELERATORS
                .iter()
                .copied()
                .map(|accel| {
                    let present = rt.is_present(accel);
                    let mut status = lw_ort::AcceleratorStatus {
                        accel,
                        present,
                        registered: false,
                        devices: 0,
                        error: None,
                    };
                    if present {
                        if accel.kind() == lw_core::capabilities::AcceleratorKind::Gpu {
                            // A status check must not keep GPU DLLs/driver heaps resident in
                            // the desktop process for the rest of its lifetime.
                            match crate::provider_worker::probe_runtime(rt.runtime_dir(), accel) {
                                Ok(count) => {
                                    status.registered = true;
                                    status.devices = count;
                                }
                                Err(error) => status.error = Some(error),
                            }
                        } else {
                            status.devices = rt.device_count(accel);
                            status.registered = rt.registered(accel);
                        }
                    }
                    status
                })
                .collect::<Vec<_>>();
            #[cfg(windows)]
            if let Some(directml) = probes
                .iter_mut()
                .find(|st| st.accel == lw_core::capabilities::Accelerator::DirectMl)
                && crate::provider_worker::directml_runtime_present(rt.runtime_dir())
            {
                directml.present = true;
                match crate::provider_worker::probe_directml(rt.runtime_dir()) {
                    Ok(count) => {
                        directml.registered = true;
                        directml.devices = count;
                        directml.error = None;
                    }
                    Err(error) => directml.error = Some(error),
                }
            }
            for st in &mut probes {
                if let Some(dir) = crate::runtime_install::installed_runtime(st.accel) {
                    st.present = true;
                    match crate::provider_worker::probe_runtime(&dir, st.accel) {
                        Ok(count) => {
                            st.registered = true;
                            st.devices = count;
                            st.error = None;
                        }
                        Err(error) => {
                            st.registered = false;
                            st.devices = 0;
                            st.error = Some(error);
                        }
                    }
                }
            }
            let mut devices = rt.device_summary();
            devices.extend(
                probes
                    .iter()
                    .filter(|st| st.usable())
                    .filter(|st| st.accel.kind() == lw_core::capabilities::AcceleratorKind::Gpu)
                    .map(|st| {
                        format!(
                            "{}: {} device(s), verified in an isolated process",
                            st.accel.label(),
                            st.devices
                        )
                    }),
            );
            let nvidia = if lw_core::capabilities::Accelerator::Cuda.supported_on_this_platform() {
                lw_ort::nvidia::devices().unwrap_or_default()
            } else {
                Vec::new()
            };
            let hardware_devices = hardware_device_descriptions(
                &devices,
                &lw_platform::caps::physical_gpu_descriptions(),
                &nvidia,
            );
            d.accelerators = probes
                .into_iter()
                .map(|st| AcceleratorStatus {
                    setup_action: crate::runtime_install::setup_action(
                        st.accel,
                        st.accel
                            .relevant_to_hardware(&machine.cpu_brand, &hardware_devices),
                        st.usable(),
                    ),
                    id: st.accel.id(),
                    label: st.accel.label(),
                    kind_label: st.accel.kind().label(),
                    vendor: st.accel.vendor(),
                    library: st.accel.library_file(),
                    needs_dedicated_artifact: st.accel.needs_dedicated_artifact(),
                    hardware_present: st
                        .accel
                        .relevant_to_hardware(&machine.cpu_brand, &hardware_devices),
                    present: st.present,
                    registered: st.registered,
                    usable: st.usable(),
                    detail: if !st.accel.supported_on_this_platform() {
                        format!(
                            "not supported by this app build ({} {})",
                            std::env::consts::OS,
                            std::env::consts::ARCH
                        )
                    } else if !st
                        .accel
                        .relevant_to_hardware(&machine.cpu_brand, &hardware_devices)
                    {
                        format!("no compatible {} detected on this machine", st.accel.label())
                    } else {
                        st.explain()
                    },
                    devices: st.devices,
                })
                .collect();
            // Provider registration adds devices to ORT's environment. Read the list only
            // after probing, or this panel shows CPU alone beside usable GPU rows.
            d.devices = devices;
        }
        Err(e) => {
            d.runtime_error = Some(e.to_string());
            d.accelerators_error = Some("ONNX Runtime not found; accelerator availability is unknown".into());
        }
    }
    d
}
