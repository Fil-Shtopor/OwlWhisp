# OwlWhisp

<p align="center">
  <img src="assets/icons/icon-256.png" width="180" alt="OwlWhisp owl and voice-wave icon">
</p>

**Your voice stays yours.** OwlWhisp is private, local speech-to-text. Windows is the primary
desktop platform; macOS and Linux packages are previews with incomplete desktop integration.
Audio is transcribed on your device. No cloud account or browser engine is required.

## What it offers

- **Private by design:** microphone audio and models stay on the device.
- **Local inference:** Parakeet runs on CPU, supported GPUs and the Qualcomm Hexagon NPU on
  compatible Snapdragon Windows machines. Other CPU models use the optional sherpa engine.
- **Desktop-native:** a Rust application with tray controls, global hotkeys, diagnostics, and no
  webview process.
- **Open source:** Apache-2.0 application code with third-party notices included in every package.

## Supported platforms

All packages are native **64-bit** builds. Choose the architecture of your computer.

| Platform | Package | Included inference | Desktop support |
|---|---|---|---|
| **Windows x64**, Intel/AMD, Windows 10/11 | NSIS installer or portable ZIP | CPU, DirectML, WebGPU; CUDA/TensorRT can be installed in Settings | Primary; verified on RTX 4080 Laptop |
| **Windows ARM64**, Windows 11, including Snapdragon X/X2 | NSIS installer or portable ZIP | CPU, WebGPU; QNN NPU on compatible Snapdragon hardware with the matching model artifact | Primary; verified on Snapdragon X2 |
| **macOS Apple Silicon**, ARM64, macOS 13.3+ | `.app` in a ZIP | CPU and bundled WebGPU/Metal provider | Preview |
| **macOS Intel**, x64, macOS 13.3+ | `.app` in a ZIP | CPU; the pinned ONNX Runtime is built from source for Intel | Preview |
| **Linux x64**, built on Ubuntu 22.04 | Portable `.tar.gz` | CPU and bundled WebGPU/Vulkan provider | Preview |
| **Linux ARM64**, built on Ubuntu 24.04 | Portable `.tar.gz` | Parakeet on CPU; sherpa models are unavailable in this package | Preview |

macOS and Linux currently support the native interface, model management, benchmarking, audio
capture and clipboard access. **Global hotkeys, typing into other applications and foreground-app
detection are not implemented there yet.** Linux packages need compatible system GTK 3 and ALSA
libraries; tray integration also depends on the desktop environment. 32-bit platforms are not built.

Provider support does not guarantee support for every GPU or model. Settings checks the physical
hardware, driver, provider and model separately. Windows x64 offers **Download runtime** for
compatible CUDA/TensorRT hardware; TensorRT selects its package by compute capability, not GPU name.
Intel/AMD NPU models and Apple CoreML/ANE are not included in the standard packages. See
[hardware documentation](docs/hardware.md) and [runtime installation](docs/build.md#hardware-selected-runtime-add-ons-windows-x64).

## Downloads

Download from [GitHub Releases](https://github.com/Fil-Shtopor/OwlWhisp/releases).
The initial **0.1.0** release is a prerelease; Windows installers and macOS bundles are unsigned.

| Platform | Portable archive | Installer |
|---|---|---|
| Windows x64 | [ZIP](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.0/OwlWhisp-0.1.0-x86_64-pc-windows-msvc.zip) | [Setup](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.0/OwlWhisp-0.1.0-x86_64-pc-windows-msvc-setup.exe) |
| Windows ARM64 | [ZIP](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.0/OwlWhisp-0.1.0-aarch64-pc-windows-msvc.zip) | [Setup](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.0/OwlWhisp-0.1.0-aarch64-pc-windows-msvc-setup.exe) |
| macOS Apple Silicon | [ZIP](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.0/OwlWhisp-0.1.0-osx-arm64-macos.zip) | — |
| macOS Intel | [ZIP](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.0/OwlWhisp-0.1.0-osx-x64-macos.zip) | — |
| Linux x64 | [tar.gz](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.0/OwlWhisp-0.1.0-linux-x64.tar.gz) | — |
| Linux ARM64 | [tar.gz](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.0/OwlWhisp-0.1.0-linux-arm64.tar.gz) | — |

Each platform has a `SHA256SUMS-*.txt` file. Release builds verify executable/runtime architecture,
model manifests and licence files, then unpack and load the packaged runtime on the native runner.
Publication requires all six platforms and all checksums. After publication, CI downloads every asset
without authentication and checks its SHA-256. Models are downloaded separately on first use.

## Hardware tests

42 simulated configurations cover Windows/macOS/Linux, x64/ARM64, CPU/GPU/NPU hardware,
missing drivers, library selection and model readiness. They run on every native CI target:

```sh
cargo test -p lw-app --test hardware_matrix
```

These tests check detection and selection policy; native GPU/NPU inference still needs hardware QA.
See [hardware-tests.md](docs/hardware-tests.md).

## Build from source

```powershell
pwsh -File scripts/runtime/fetch-runtime.ps1 -Platform win-x64
$env:SHERPA_ONNX_LIB_DIR = (pwsh -File scripts/build/fetch-sherpa.ps1 -Platform win-x64 -Quiet)
cargo build --release -p lw-gui --features sherpa
```

This example needs a Windows x64 MSVC environment. Change the platform/target for other hosts.
Omit `--features sherpa` for a Parakeet-only development build; Linux ARM64 uses that profile.

Run `target/release/owlwhisp` (or `owlwhisp.exe` on Windows). For platform-specific build,
runtime, and packaging instructions, see [docs/build.md](docs/build.md). The documentation index
is in [docs/README.md](docs/README.md).

## Licensing

Application code is **Apache-2.0**. Model and runtime licences are documented in
[docs/licenses.md](docs/licenses.md) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
