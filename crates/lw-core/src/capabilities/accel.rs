//! The accelerators LocalWisper knows how to ask ONNX Runtime for.
//!
//! One vocabulary, used by settings (what the user picked), detection (what this machine has),
//! the engine (what it will try), and the UI (what actually ran). Adding a vendor means adding a
//! variant here and staging its execution-provider library — not touching the audio path, the
//! text pipeline or the UI.
//!
//! Every accelerator except [`Accelerator::Cpu`] reaches ONNX Runtime through the **plugin
//! execution-provider** mechanism (`RegisterExecutionProviderLibrary`), which is the same
//! mechanism the verified Qualcomm NPU path already uses. That is why the list can grow without
//! rebuilding ONNX Runtime: a vendor ships a provider library, we register it by name.
//!
//! Two things a variant here does **not** promise:
//!
//! 1. **That the library is installed.** Each one ships as its own redistributable, and several
//!    (CUDA, OpenVINO, Vitis AI) additionally need a vendor SDK on the machine.
//! 2. **That a model can use it.** NPUs need a model artifact quantized and compiled for that
//!    specific NPU; GPUs generally run the ordinary graph. [`Accelerator::needs_dedicated_artifact`]
//!    is the difference, and it is the reason GPU support generalizes and NPU support does not.

use serde::{Deserialize, Serialize};

/// Broad class of compute an accelerator provides, for grouping in the UI and for coarse
/// preferences like "use the GPU, whichever vendor it is".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceleratorKind {
    /// Ordinary CPU execution.
    Cpu,
    /// A graphics processor used for compute.
    Gpu,
    /// A dedicated neural accelerator.
    Npu,
}

impl AcceleratorKind {
    /// Human label.
    pub fn label(self) -> &'static str {
        match self {
            AcceleratorKind::Cpu => "CPU",
            AcceleratorKind::Gpu => "GPU",
            AcceleratorKind::Npu => "NPU",
        }
    }
}

/// An execution provider LocalWisper can run the encoder on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accelerator {
    /// ONNX Runtime's built-in CPU execution provider. Always present, every platform.
    Cpu,
    /// Qualcomm Hexagon NPU via the QNN execution provider (Snapdragon X / X2).
    QnnNpu,
    /// Any GPU, through ONNX Runtime's WebGPU plugin EP (Dawn: D3D12 on Windows, Vulkan on
    /// Linux, Metal on macOS). Vendor-neutral, and needs no model artifact of its own.
    WebGpu,
    /// NVIDIA GPU via the CUDA execution provider.
    Cuda,
    /// NVIDIA GPU via TensorRT (faster than CUDA, longer first-run build).
    TensorRt,
    /// Any Direct3D 12 GPU on Windows via the DirectML execution provider.
    DirectMl,
    /// Apple Neural Engine / GPU via the CoreML execution provider.
    CoreMl,
    /// Intel CPU / GPU / NPU via the OpenVINO execution provider.
    OpenVino,
    /// AMD XDNA NPU (Ryzen AI) via the Vitis AI execution provider.
    VitisAi,
}

/// Every accelerator, in the order the automatic policy prefers them.
///
/// NPUs first (lowest power for this workload and, on the verified Snapdragon path, the fastest),
/// then GPUs, then CPU. Within GPUs, vendor-specific providers come before the portable one
/// because they are generally faster when they are actually available.
pub const ALL_ACCELERATORS: [Accelerator; 9] = [
    Accelerator::QnnNpu,
    Accelerator::OpenVino,
    Accelerator::VitisAi,
    Accelerator::CoreMl,
    Accelerator::TensorRt,
    Accelerator::Cuda,
    Accelerator::DirectMl,
    Accelerator::WebGpu,
    Accelerator::Cpu,
];

impl Accelerator {
    /// Stable machine-readable id, matching the serde name (`"qnn_npu"`, `"web_gpu"`, …).
    pub fn id(self) -> &'static str {
        match self {
            Accelerator::Cpu => "cpu",
            Accelerator::QnnNpu => "qnn_npu",
            Accelerator::WebGpu => "web_gpu",
            Accelerator::Cuda => "cuda",
            Accelerator::TensorRt => "tensor_rt",
            Accelerator::DirectMl => "direct_ml",
            Accelerator::CoreMl => "core_ml",
            Accelerator::OpenVino => "open_vino",
            Accelerator::VitisAi => "vitis_ai",
        }
    }

    /// Parse an [`Accelerator::id`] back.
    pub fn from_id(s: &str) -> Option<Self> {
        ALL_ACCELERATORS.iter().copied().find(|a| a.id() == s)
    }

    /// Short human label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Accelerator::Cpu => "CPU",
            Accelerator::QnnNpu => "Qualcomm NPU (Hexagon)",
            Accelerator::WebGpu => "GPU (WebGPU)",
            Accelerator::Cuda => "NVIDIA GPU (CUDA)",
            Accelerator::TensorRt => "NVIDIA GPU (TensorRT)",
            Accelerator::DirectMl => "GPU (DirectML)",
            Accelerator::CoreMl => "Apple Neural Engine / GPU",
            Accelerator::OpenVino => "Intel CPU / GPU / NPU (OpenVINO)",
            Accelerator::VitisAi => "AMD NPU (Ryzen AI)",
        }
    }

    /// Which vendor's hardware this targets, or `None` when it is vendor-neutral.
    pub fn vendor(self) -> Option<&'static str> {
        match self {
            Accelerator::QnnNpu => Some("Qualcomm"),
            Accelerator::Cuda | Accelerator::TensorRt => Some("NVIDIA"),
            Accelerator::CoreMl => Some("Apple"),
            Accelerator::OpenVino => Some("Intel"),
            Accelerator::VitisAi => Some("AMD"),
            Accelerator::Cpu | Accelerator::WebGpu | Accelerator::DirectMl => None,
        }
    }

    /// Broad compute class.
    ///
    /// OpenVINO is reported as an NPU because that is why we would choose it over the CPU EP on
    /// an Intel machine; it can also target Intel CPUs and GPUs, which the EP decides internally.
    pub fn kind(self) -> AcceleratorKind {
        match self {
            Accelerator::Cpu => AcceleratorKind::Cpu,
            Accelerator::QnnNpu | Accelerator::OpenVino | Accelerator::VitisAi => AcceleratorKind::Npu,
            Accelerator::WebGpu
            | Accelerator::Cuda
            | Accelerator::TensorRt
            | Accelerator::DirectMl
            | Accelerator::CoreMl => AcceleratorKind::Gpu,
        }
    }

    /// The name ONNX Runtime registers this provider under, and reports on enumerated devices.
    ///
    /// `None` for the CPU EP, which is built in and never registered.
    pub fn ep_name(self) -> Option<&'static str> {
        match self {
            Accelerator::Cpu => None,
            Accelerator::QnnNpu => Some("QNNExecutionProvider"),
            Accelerator::WebGpu => Some("WebGpuExecutionProvider"),
            Accelerator::Cuda => Some("CUDAExecutionProvider"),
            Accelerator::TensorRt => Some("TensorrtExecutionProvider"),
            Accelerator::DirectMl => Some("DmlExecutionProvider"),
            Accelerator::CoreMl => Some("CoreMLExecutionProvider"),
            Accelerator::OpenVino => Some("OpenVINOExecutionProvider"),
            Accelerator::VitisAi => Some("VitisAIExecutionProvider"),
        }
    }

    /// The provider library file name to look for in the runtime directory, per platform.
    ///
    /// `None` means this accelerator cannot be loaded as a plugin on the current platform — either
    /// it is built in (CPU) or the vendor does not ship a plugin library for this OS.
    pub fn library_file(self) -> Option<&'static str> {
        let windows = cfg!(target_os = "windows");
        let macos = cfg!(target_os = "macos");
        match self {
            Accelerator::Cpu => None,
            // Qualcomm ships Windows-only; Snapdragon Linux is not a target we stage for.
            Accelerator::QnnNpu => windows.then_some("onnxruntime_providers_qnn.dll"),
            Accelerator::WebGpu => Some(if windows {
                "onnxruntime_providers_webgpu.dll"
            } else if macos {
                "libonnxruntime_providers_webgpu.dylib"
            } else {
                "libonnxruntime_providers_webgpu.so"
            }),
            Accelerator::Cuda => Some(if windows {
                "onnxruntime_providers_cuda.dll"
            } else {
                "libonnxruntime_providers_cuda.so"
            }),
            Accelerator::TensorRt => Some(if windows {
                "onnxruntime_providers_tensorrt.dll"
            } else {
                "libonnxruntime_providers_tensorrt.so"
            }),
            Accelerator::DirectMl => windows.then_some("onnxruntime_providers_dml.dll"),
            Accelerator::CoreMl => macos.then_some("libonnxruntime_providers_coreml.dylib"),
            Accelerator::OpenVino => Some(if windows {
                "onnxruntime_providers_openvino.dll"
            } else {
                "libonnxruntime_providers_openvino.so"
            }),
            Accelerator::VitisAi => windows.then_some("onnxruntime_providers_vitisai.dll"),
        }
    }

    /// Whether this accelerator needs a model artifact built specifically for it.
    ///
    /// This is the line between "supporting a GPU" and "supporting an NPU". A GPU consumes the
    /// ordinary ONNX graph, so one artifact serves every vendor. An NPU wants the graph quantized
    /// and compiled for that silicon, which must be produced and validated on that silicon — so
    /// an NPU is never supported by writing code alone.
    pub fn needs_dedicated_artifact(self) -> bool {
        matches!(
            self,
            Accelerator::QnnNpu | Accelerator::VitisAi | Accelerator::OpenVino
        )
    }

    /// Whether this accelerator can exist at all on the platform this binary was built for.
    pub fn supported_on_this_platform(self) -> bool {
        self == Accelerator::Cpu || self.library_file().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for a in ALL_ACCELERATORS {
            assert!(seen.insert(a.id()), "duplicate id {}", a.id());
            assert_eq!(Accelerator::from_id(a.id()), Some(a));
        }
        assert_eq!(Accelerator::from_id("nope"), None);
    }

    #[test]
    fn ids_match_the_serde_representation() {
        // Settings persist the serde form; the UI and CLI use `id()`. They must not diverge.
        for a in ALL_ACCELERATORS {
            let json = serde_json::to_string(&a).unwrap();
            assert_eq!(json, format!("\"{}\"", a.id()), "{a:?}");
        }
    }

    #[test]
    fn cpu_is_the_only_built_in_provider() {
        for a in ALL_ACCELERATORS {
            if a == Accelerator::Cpu {
                assert_eq!(a.ep_name(), None);
                assert_eq!(a.library_file(), None);
            } else {
                assert!(a.ep_name().is_some(), "{a:?} needs a registration name");
            }
        }
    }

    #[test]
    fn automatic_order_puts_npus_first_and_cpu_last() {
        let kinds: Vec<_> = ALL_ACCELERATORS.iter().map(|a| a.kind()).collect();
        let first_gpu = kinds.iter().position(|k| *k == AcceleratorKind::Gpu).unwrap();
        let last_npu = kinds.iter().rposition(|k| *k == AcceleratorKind::Npu).unwrap();
        assert!(last_npu < first_gpu, "every NPU must precede every GPU");
        assert_eq!(*ALL_ACCELERATORS.last().unwrap(), Accelerator::Cpu);
    }

    #[test]
    fn only_npu_targets_need_their_own_artifact() {
        // The claim this rests on: a GPU EP consumes the ordinary graph, an NPU EP does not.
        for a in ALL_ACCELERATORS {
            match a.kind() {
                AcceleratorKind::Npu => assert!(a.needs_dedicated_artifact(), "{a:?}"),
                _ => assert!(!a.needs_dedicated_artifact(), "{a:?}"),
            }
        }
    }

    #[test]
    fn cpu_is_supported_everywhere() {
        assert!(Accelerator::Cpu.supported_on_this_platform());
        // WebGPU is the portable one: it must be loadable on every platform we build for.
        assert!(Accelerator::WebGpu.supported_on_this_platform());
    }

    #[test]
    fn platform_only_providers_are_absent_off_platform() {
        if cfg!(target_os = "windows") {
            assert!(Accelerator::QnnNpu.library_file().is_some());
            assert!(Accelerator::CoreMl.library_file().is_none());
        }
        if cfg!(target_os = "macos") {
            assert!(Accelerator::CoreMl.library_file().is_some());
            assert!(Accelerator::DirectMl.library_file().is_none());
        }
    }
}
