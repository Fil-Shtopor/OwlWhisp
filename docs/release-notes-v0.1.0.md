# OwlWhisp 0.1.0 preview

Native packages for Windows x64/ARM64, macOS Apple Silicon/Intel and Linux x64/ARM64.
Windows includes an installer and portable ZIP; macOS includes an `.app` ZIP; Linux includes a
portable tarball. Each platform has SHA-256 checksums.

- Windows is the primary desktop platform. macOS/Linux are previews: global hotkeys, automatic
  text injection and foreground-app detection are still incomplete.
- Windows x64 includes CPU, DirectML and WebGPU. Settings installs the matching pinned CUDA or
  TensorRT packages for compatible NVIDIA hardware. A driver update is offered separately.
- Windows ARM64 includes QNN for compatible Qualcomm hardware; a matching NPU model is required.
- macOS Intel includes ONNX Runtime 1.28.1 built from its pinned upstream source.
- Linux ARM64 supports Parakeet on CPU. Other CPU models require sherpa, which is omitted because
  the pinned upstream version does not publish a no-TTS ARM64 Linux package.
- Models are downloaded in the application; model weights are not included in the archives.
- 42 simulated hardware configurations check detection, library selection and model readiness.
- Every release archive is unpacked on its native runner and checked for architecture, manifests,
  licences, engine features and actual loading of the bundled CPU runtime before publication.

Windows installers and macOS bundles are unsigned. Linux packages require compatible GTK 3/ALSA
system libraries. See the README for platform details and download names.
