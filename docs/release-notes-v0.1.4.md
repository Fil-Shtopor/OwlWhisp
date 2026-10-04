# OwlWhisp 0.1.4 preview

This release adds a native Linux ARM64 CUDA runtime for NVIDIA DGX Spark and explains accelerator
selection and benchmarking in the README.

- **Linux ARM64 / DGX Spark:** the package now includes ONNX Runtime 1.30.0's matched native core
  and CUDA 13 provider with GB10 / SM 12.1 kernels. Choose **NVIDIA GPU (CUDA)** for Parakeet.
  Compatible system NVIDIA driver, CUDA 13 libraries and cuDNN 9 are required. Python is not
  required to run the app. [Setup guide](https://github.com/Fil-Shtopor/OwlWhisp/blob/v0.1.4/docs/build.md#nvidia-dgx-spark-linux-arm64).
- The ARM64 GPU archive is pinned by size and SHA-256; staging validates library architecture,
  extracts only native C libraries and carries the matching upstream licence notices. Release
  verification requires the CUDA/shared providers and checks versioned Linux libraries too.
- Hardware tests now include GB10 with a display, headless GB10 and an older driver. Native ARM64
  CI verifies the packaged runtime on CPU. **GPU inference and speed on a physical Spark remain
  untested.** TensorRT is not included in the ARM64 runtime.
- Intel macOS reuses the unchanged, source-built ONNX Runtime 1.28.1 from a hash-pinned earlier
  release on a cache miss; native package loading is still verified before publication.
- **README:** accelerator support/requirements, accelerator comparisons on shared audio and model
  benchmarking with cold/warm RTF and word error rate are now prominent documented features.
- Includes 0.1.3's **Settings > Startup > Start minimized to tray** option: save it and the next
  launch keeps the main window hidden, including at login. Open the window from the tray; if no
  tray is available the main window opens normally. The option defaults to off.

Windows CUDA/TensorRT installation remains available in Settings. GPU/NPU acceleration applies
to Parakeet; other catalog models run on CPU where sherpa is included. macOS/Linux desktop
integration remains in preview. Windows installers and macOS bundles remain unsigned.
