//! Model/provider readiness policy, shared by the GUI and simulated machine tests.
use lw_core::capabilities::{OperatingSystem, Platform};

/// Use the engine's file requirements for the Qualcomm NPU readiness display.
pub fn npu_model_ready(model_dir: &std::path::Path) -> bool {
    lw_engine_parakeet::npu::model_ready(model_dir, 2000)
}

/// Hardware-specific preview status, independent of whether a provider currently loads.
pub fn experimental_gpu_note_on(platform: Platform, cpu: &str, devices: &[String]) -> Option<&'static str> {
    (platform.os == OperatingSystem::Windows
        && platform.arch == lw_core::capabilities::Architecture::Arm64
        && (cpu.to_ascii_lowercase().contains("nvidia")
            || devices.iter().any(|d| {
                let d = d.to_ascii_lowercase();
                d.contains("nvidia") || d.contains("ven_10de")
            })))
    .then_some("RTX Spark / NVIDIA Windows ARM64: experimental, not tested on a physical RTX Spark. Use DirectML or WebGPU with Parakeet TDT 0.6B v3; CUDA and TensorRT runtimes are not included for Windows ARM64.")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Whether the selected model can run with this provider on the target machine.
pub enum ModelAvailability {
    Available,
    Unavailable,
    NeedAdditionalAction,
}

impl ModelAvailability {
    pub fn label(self) -> &'static str {
        match self {
            Self::Available => "Available",
            Self::Unavailable => "Unavailable",
            Self::NeedAdditionalAction => "Need additional action",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Concrete setup operations which Settings can offer immediately.
pub enum AcceleratorAction {
    PrepareModel,
    DownloadRuntime,
    InstallDriver,
}

#[derive(Clone, Debug, Default)]
/// Independent model, hardware and provider observations used by the readiness policy.
pub struct AcceleratorReadiness {
    pub parakeet: bool,
    pub model_ready: bool,
    pub gpu_model_ready: bool,
    pub npu_model_ready: bool,
    pub probe_ok: bool,
    pub hardware_present: bool,
    pub provider_registered: bool,
    pub runtime_installable: bool,
    pub driver_needed: bool,
    pub usable: bool,
    pub model_check_failed: bool,
    pub driver_install_failed: bool,
}

/// Evaluate readiness for an explicit target. Every action-required verdict has an action.
pub fn model_availability_on(
    platform: Platform,
    accel: lw_core::capabilities::Accelerator,
    r: &AcceleratorReadiness,
) -> (ModelAvailability, Option<AcceleratorAction>) {
    use ModelAvailability as Status;
    use lw_core::capabilities::{Accelerator as A, AcceleratorKind};

    if !r.probe_ok || !accel.supported_on(platform) || !r.model_ready {
        return (Status::Unavailable, None);
    }
    if !r.parakeet && accel != A::Cpu {
        return (Status::Unavailable, None);
    }
    if accel.kind() == AcceleratorKind::Npu && accel != A::QnnNpu {
        // Other NPU backends have no artifact this app can supply.
        return (Status::Unavailable, None);
    }
    if !r.hardware_present {
        return (Status::Unavailable, None);
    }
    if !r.usable {
        if r.runtime_installable {
            return (
                Status::NeedAdditionalAction,
                Some(AcceleratorAction::DownloadRuntime),
            );
        }
        if r.driver_install_failed {
            return (Status::Unavailable, None);
        }
        let driver_installable = platform.os == OperatingSystem::Windows
            && (r.provider_registered || r.driver_needed)
            && matches!(accel, A::Cuda | A::TensorRt | A::WebGpu | A::DirectMl);
        return if driver_installable {
            (
                Status::NeedAdditionalAction,
                Some(AcceleratorAction::InstallDriver),
            )
        } else {
            (Status::Unavailable, None)
        };
    }
    if r.model_check_failed {
        // The provider enumerated, but the real model check failed. Keep retry available while
        // reporting the current state honestly.
        return (
            Status::NeedAdditionalAction,
            Some(AcceleratorAction::PrepareModel),
        );
    }
    if (accel.kind() == AcceleratorKind::Gpu && !r.gpu_model_ready)
        || (accel == A::QnnNpu && !r.npu_model_ready)
    {
        return (
            Status::NeedAdditionalAction,
            Some(AcceleratorAction::PrepareModel),
        );
    }
    (Status::Available, None)
}

/// Apply the readiness policy on the current executable platform.
pub fn model_availability(
    accel: lw_core::capabilities::Accelerator,
    r: &AcceleratorReadiness,
) -> (ModelAvailability, Option<AcceleratorAction>) {
    model_availability_on(Platform::current(), accel, r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lw_core::capabilities::Accelerator;
    #[test]
    fn model_status_has_a_working_action_or_is_unavailable() {
        let mut ready = AcceleratorReadiness {
            parakeet: true,
            model_ready: true,
            gpu_model_ready: false,
            npu_model_ready: false,
            probe_ok: true,
            hardware_present: true,
            provider_registered: true,
            runtime_installable: false,
            driver_needed: false,
            usable: true,
            model_check_failed: false,
            driver_install_failed: false,
        };
        assert_eq!(
            model_availability_on(Platform::from_names("windows", "x64"), Accelerator::Cpu, &ready),
            (ModelAvailability::Available, None)
        );
        ready.model_ready = false;
        assert_eq!(
            model_availability_on(Platform::from_names("windows", "x64"), Accelerator::Cpu, &ready),
            (ModelAvailability::Unavailable, None)
        );
        ready.model_ready = true;
        assert_eq!(
            model_availability_on(Platform::from_names("windows", "x64"), Accelerator::Cuda, &ready),
            (
                ModelAvailability::NeedAdditionalAction,
                Some(AcceleratorAction::PrepareModel)
            )
        );
        assert_eq!(
            model_availability_on(
                Platform::from_names("windows", "arm64"),
                Accelerator::QnnNpu,
                &ready
            ),
            (
                ModelAvailability::NeedAdditionalAction,
                Some(AcceleratorAction::PrepareModel)
            ),
            "QNN can prepare the pinned FP32 encoder after downloading it"
        );
        ready.npu_model_ready = true;
        assert_eq!(
            model_availability_on(
                Platform::from_names("windows", "arm64"),
                Accelerator::QnnNpu,
                &ready
            ),
            (ModelAvailability::Available, None)
        );
        ready.parakeet = false;
        assert_eq!(
            model_availability_on(Platform::from_names("windows", "x64"), Accelerator::Cuda, &ready),
            (ModelAvailability::Unavailable, None),
            "the CPU-only sherpa engine cannot use a GPU add-on"
        );
        ready.parakeet = true;
        ready.usable = false;
        assert_eq!(
            model_availability_on(Platform::from_names("windows", "x64"), Accelerator::Cuda, &ready),
            (
                ModelAvailability::NeedAdditionalAction,
                Some(AcceleratorAction::InstallDriver),
            )
        );
        ready.provider_registered = false;
        assert_eq!(
            model_availability_on(Platform::from_names("windows", "x64"), Accelerator::Cuda, &ready),
            (ModelAvailability::Unavailable, None),
            "a display driver does not supply a missing ONNX provider"
        );
        ready.runtime_installable = true;
        assert_eq!(
            model_availability_on(Platform::from_names("windows", "x64"), Accelerator::Cuda, &ready),
            (
                ModelAvailability::NeedAdditionalAction,
                Some(AcceleratorAction::DownloadRuntime)
            ),
            "a missing provider on compatible hardware must have a package download action"
        );
        ready.hardware_present = false;
        assert_eq!(
            model_availability_on(Platform::from_names("windows", "x64"), Accelerator::Cuda, &ready),
            (ModelAvailability::Unavailable, None)
        );
        ready.hardware_present = true;
        ready.runtime_installable = false;
        ready.provider_registered = true;
        ready.driver_install_failed = true;
        assert_eq!(
            model_availability_on(Platform::from_names("windows", "x64"), Accelerator::Cuda, &ready),
            (ModelAvailability::Unavailable, None),
            "a failed driver installation must not promise another download"
        );
        ready.driver_install_failed = false;
        ready.usable = true;
        ready.gpu_model_ready = true;
        ready.model_check_failed = true;
        assert_eq!(
            model_availability_on(Platform::from_names("windows", "x64"), Accelerator::Cuda, &ready),
            (
                ModelAvailability::NeedAdditionalAction,
                Some(AcceleratorAction::PrepareModel)
            ),
            "a failed model run must not be reported as available"
        );
    }
}
