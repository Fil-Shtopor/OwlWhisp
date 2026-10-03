//! The accelerators OwlWhisp knows how to ask ONNX Runtime for.
//!
//! One vocabulary, used by settings (what the user picked), detection (what this machine has),
//! the engine (what it will try), and the UI (what actually ran). Adding a vendor means adding a
//! variant here and staging its execution-provider library — not touching the audio path, the
//! text pipeline or the UI.
//!
//! Most accelerators reach ONNX Runtime through the plugin execution-provider mechanism
//! (`RegisterExecutionProviderLibrary`). The Windows x64 CUDA package is an exception: its
//! provider is built into a matched GPU edition of ONNX Runtime and added to each CUDA session.
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

/// An execution provider OwlWhisp can run the encoder on.
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
/// **NPUs, then the CPU, then GPUs** — and the CPU's position is deliberate.
///
/// An NPU is first because it is what this workload is shaped for: on the verified Snapdragon
/// path it is both the fastest and the lowest-power option by a wide margin (RTF 0.0160 against
/// 0.0324 on the CPU).
///
/// A GPU comes *after* the CPU in this hardware-only fallback because speed also depends on the
/// model graph. With the bundled Parakeet INT8 encoder, CUDA on an RTX 4080 was about equal to a
/// Core i9 CPU; with the optional full-precision encoder it was about 8x faster. Settings offers
/// that model download and selects the GPU after a successful test. A GPU is also selectable for
/// other verified model/provider combinations rather than relying on this static order.
///
/// Within GPUs, vendor-specific providers precede the portable one: when CUDA or CoreML is
/// actually present it is the better-optimized path.
pub const ALL_ACCELERATORS: [Accelerator; 9] = [
    Accelerator::QnnNpu,
    Accelerator::OpenVino,
    Accelerator::VitisAi,
    Accelerator::Cpu,
    Accelerator::CoreMl,
    Accelerator::TensorRt,
    Accelerator::Cuda,
    Accelerator::DirectMl,
    Accelerator::WebGpu,
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
            Accelerator::OpenVino => "Intel NPU (OpenVINO)",
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
            // Qualcomm ships QNN for Windows on Snapdragon ARM64 only.
            Accelerator::QnnNpu => cfg!(all(target_os = "windows", target_arch = "aarch64"))
                .then_some("onnxruntime_providers_qnn.dll"),
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
            // DirectML is compiled into the Windows ML ONNX Runtime core, not a plugin DLL.
            Accelerator::DirectMl => None,
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
        match self {
            Accelerator::Cpu => true,
            Accelerator::DirectMl => cfg!(target_os = "windows"),
            _ => self.library_file().is_some(),
        }
    }

    /// Whether this provider is relevant to the physical hardware detected on a machine.
    /// `devices` is the post-probe ONNX Runtime device summary; `cpu_brand` comes from the OS.
    /// This intentionally differs from [`Self::supported_on_this_platform`], which only answers
    /// whether the provider can run on this OS/architecture.
    pub fn relevant_to_hardware(self, cpu_brand: &str, devices: &[String]) -> bool {
        let cpu = cpu_brand.to_ascii_lowercase();
        let devices: Vec<String> = devices.iter().map(|d| d.to_ascii_lowercase()).collect();
        let has_gpu = devices.iter().any(|d| d.contains(" gpu "));
        let has_nvidia = devices.iter().any(|d| d.contains("nvidia") && d.contains(" gpu "));
        // This provider is currently probed for an NPU, so an ordinary Intel CPU or integrated
        // GPU must not be presented as an available OpenVINO NPU.
        let has_intel_npu = cpu.contains("core ultra")
            || devices.iter().any(|d| d.contains("intel") && d.contains(" npu "));
        match self {
            Accelerator::Cpu => true,
            Accelerator::QnnNpu => {
                cfg!(all(target_os = "windows", target_arch = "aarch64"))
                    && (cpu.contains("qualcomm")
                        || cpu.contains("snapdragon")
                        || cpu.contains("oryon"))
            }
            Accelerator::VitisAi => cpu.contains("ryzen ai") || cpu.contains("xdna"),
            Accelerator::OpenVino => has_intel_npu,
            Accelerator::Cuda | Accelerator::TensorRt => has_nvidia,
            Accelerator::DirectMl | Accelerator::WebGpu => has_gpu,
            Accelerator::CoreMl => cfg!(target_os = "macos"),
        }
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
    fn automatic_order_is_npu_then_cpu_then_gpu() {
        // The CPU's position is evidence-based, not an oversight: see the constant's docs.
        let kinds: Vec<_> = ALL_ACCELERATORS.iter().map(|a| a.kind()).collect();
        let last_npu = kinds.iter().rposition(|k| *k == AcceleratorKind::Npu).unwrap();
        let cpu = kinds.iter().position(|k| *k == AcceleratorKind::Cpu).unwrap();
        let first_gpu = kinds.iter().position(|k| *k == AcceleratorKind::Gpu).unwrap();
        assert!(last_npu < cpu, "every NPU must precede the CPU");
        assert!(cpu < first_gpu, "the CPU must precede every GPU");
    }

    #[test]
    fn automatic_never_leaves_the_cpu_out() {
        // Whatever the order, Auto must always have a fallback that works everywhere.
        assert!(ALL_ACCELERATORS.contains(&Accelerator::Cpu));
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
            assert_eq!(
                Accelerator::QnnNpu.library_file().is_some(),
                cfg!(target_arch = "aarch64")
            );
            assert!(Accelerator::CoreMl.library_file().is_none());
        }
        if cfg!(target_os = "macos") {
            assert!(Accelerator::CoreMl.library_file().is_some());
            assert!(Accelerator::DirectMl.library_file().is_none());
        }
    }

    #[test]
    fn hardware_relevance_uses_cpu_brand_and_gpu_adapters() {
        let devices = vec![
            "WebGpuExecutionProvider: NVIDIA GPU (id 10208)".into(),
            "WebGpuExecutionProvider: Intel Corporation GPU (id 42888)".into(),
        ];
        assert!(!Accelerator::QnnNpu.relevant_to_hardware("Intel Core i9", &devices));
        assert!(!Accelerator::VitisAi.relevant_to_hardware("Intel Core i9", &devices));
        assert!(Accelerator::TensorRt.relevant_to_hardware("Intel Core i9", &devices));
        assert!(!Accelerator::OpenVino.relevant_to_hardware("Intel Core i9", &devices));
        assert!(Accelerator::OpenVino.relevant_to_hardware("Intel Core Ultra 7", &devices));
        assert!(Accelerator::WebGpu.relevant_to_hardware("Intel Core i9", &devices));
        let physical = vec!["Windows display: NVIDIA GeForce RTX 4080 GPU (PCI\\VEN_10DE)".into()];
        assert!(Accelerator::Cuda.relevant_to_hardware("Intel Core i9", &physical));
        assert!(Accelerator::VitisAi.relevant_to_hardware("AMD Ryzen AI 9", &[]));
        assert_eq!(
            Accelerator::QnnNpu.relevant_to_hardware("Qualcomm Snapdragon X", &[]),
            cfg!(all(target_os = "windows", target_arch = "aarch64"))
        );
    }
}
