# OwlWhisp

<p align="center">
  <img src="assets/icons/icon-256.png" width="180" alt="OwlWhisp owl and voice-wave icon">
</p>

**Your voice stays yours.** OwlWhisp is private, local speech-to-text. Windows is the primary
desktop platform; macOS and Linux packages are previews with incomplete desktop integration.
Audio is transcribed on your device. No cloud account or browser engine is required.
Built to stay out of your way: a native Rust client with a small background footprint when the
speech model is unloaded.

## What it offers

- **Private by design:** microphone audio and models stay on the device.
- **Local inference:** Parakeet runs on CPU, supported GPUs and the Qualcomm Hexagon NPU on
  compatible Snapdragon Windows machines. Other CPU models use the optional sherpa engine.
- **Choose your accelerator:** select CPU, NVIDIA CUDA/TensorRT, DirectML, WebGPU or Qualcomm
  Hexagon NPU where supported. Settings shows hardware, runtime and model readiness, with setup
  actions for available downloads. See the [accelerator table](#accelerators).
- **Benchmark on your hardware:** compare accelerators on the same audio and benchmark different
  models to choose your own balance of speed and accuracy. Results include the accelerator that
  actually ran, cold/warm speed and word error rate. See [benchmarking](#benchmark-models-and-accelerators).
- **Desktop-native:** a Rust application with tray controls, global hotkeys, diagnostics, and no
  webview process.
- **Lightweight by design:** a CPU-rendered native interface without Electron or a browser engine.
  Version 0.1.1 lets you release the speech model after an idle timeout or keep it ready
  for faster responses.
- **Open source:** Apache-2.0 application code with third-party notices included in every package.

## Accelerators

In **Settings > Accelerator**, choose an explicit accelerator or use **Automatic**, **Any GPU**
or **Any NPU**. An explicit benchmark choice reports an error when it cannot run. If a saved GPU
choice becomes unavailable, dictation recovers on CPU and displays the reason without changing
your preference. Settings explains missing hardware, drivers, runtime libraries or model files.

| Accelerator | Platforms in the release | Requirements and status |
|---|---|---|
| **CPU** | Windows, macOS and Linux, x64/ARM64 | All packaged models where the engine is included; Linux ARM64 has Parakeet only |
| **NVIDIA GPU (CUDA)** | Windows x64; Linux ARM64, including DGX Spark | Windows installs the runtime in Settings. Linux ARM64 bundles the CUDA 13 provider and needs system CUDA 13/cuDNN 9 libraries; Spark hardware validation is pending |
| **NVIDIA GPU (TensorRT)** | Windows x64 | Optional runtime download selected by GPU compute capability; the first run builds an engine cache. Not bundled for DGX Spark |
| **GPU (DirectML)** | Windows x64/ARM64 | Compatible DirectX 12 GPU, including NVIDIA, AMD and Intel; bundled runtime; RTX Spark untested |
| **GPU (WebGPU)** | Windows x64/ARM64, macOS Apple Silicon, Linux x64 | Bundled provider using Direct3D 12, Metal or Vulkan, respectively; compatible GPU/driver required |
| **Qualcomm NPU (Hexagon/QNN)** | Windows ARM64 on compatible Snapdragon | Matching QNN driver; Prepare NPU model downloads and prepares the encoder; X2 previously verified |

GPU/NPU acceleration currently applies to **Parakeet TDT 0.6B v3**. The other catalog models use
CPU through sherpa. CoreML/Apple Neural Engine, Intel OpenVINO NPU and AMD Ryzen AI/Vitis AI are
not included in the standard release. A supported provider still needs a compatible device and
model; [hardware documentation](docs/hardware.md) distinguishes implemented paths from measured
hardware results.

### NVIDIA DGX Spark

Starting with **0.1.4**, the Linux ARM64 archive includes a matched ONNX Runtime **1.30.0** core
and **CUDA 13** provider, including kernels for GB10's **compute capability 12.1**. Spark combines
an Arm CPU with a Blackwell GPU; use the **Linux ARM64** package and select **NVIDIA GPU (CUDA)**.
Install compatible system CUDA 13 and cuDNN 9 libraries, then check Diagnostics and run a Parakeet
benchmark. See the [Spark setup guide](docs/build.md#nvidia-dgx-spark-linux-arm64).

This path is implemented with native ARM64 package checks and simulated GB10 detection tests;
GPU inference and performance have **not yet been measured on a physical Spark**. TensorRT is
not included in the ARM64 runtime. Linux desktop integration retains the preview limitations below.
Hardware details: [NVIDIA Spark specifications](https://docs.nvidia.com/dgx/dgx-spark/hardware.html)
and [CUDA compute capabilities](https://developer.nvidia.com/cuda/gpus).

### NVIDIA RTX Spark (Windows ARM64)

Starting with **0.1.5**, the Windows ARM64 package includes **DirectML** as well as **WebGPU**.
Use the ARM64 installer, a compatible NVIDIA Windows driver, **Parakeet TDT 0.6B v3**, and select
**GPU (DirectML)** or **GPU (WebGPU)** in Settings. The app checks the provider and GPU model
before offering it as available. Run Benchmark to compare the two accelerators on your device.

**Experimental, not tested on a physical RTX Spark.** Native ARM64 library checks and simulated
RTX Spark/N1X detection tests are included. CUDA and classic TensorRT runtime downloads remain
Windows x64 only; they are not claimed for Windows ARM64. RTX Spark's Windows platform is
documented in [NVIDIA's porting guide](https://docs.nvidia.com/rtx-spark/rtx-spark-porting-guide/latest/overview.html).
DGX Spark uses the separate Linux ARM64 path above.

## Benchmark models and accelerators

Open **Benchmark**, choose a model and run **Run benchmark** for one accelerator, or
**Compare all accelerators** to measure every usable accelerator for that model side by side.
Every package includes 15 real speech clips with reference transcripts. The comparison reuses
one audio set, including separately installed accelerator runtimes, so speed and accuracy
results are comparable. Providers that cannot run are listed with their reasons.

- **Cold RTF** measures the first clip, including warm-up; **warm RTF** measures subsequent clips.
  RTF is processing time divided by audio duration: lower is faster, and below 1 means faster
  than real time.
- **WER (word error rate)** measures transcription errors: lower is more accurate. Only languages
  advertised by the model are scored; the 15-clip fixture set covers English, Russian, Spanish, Ukrainian and Chinese. Chinese uses
  **CER (character error rate)**, reported separately from WER.
- Reports name the **actual accelerator**, explain fallback/failure and mark the fastest and
  most accurate results. Those can be different choices.

To compare models, select each model and benchmark it on the same accelerator and fixture set.
Measured results appear in the model catalog; compare models that support the language you need.
The accelerator sweep compares one selected model at a time. First runs may take longer while
model files or GPU/NPU caches are prepared. [Benchmark method and usage](docs/using.md#6-benchmarking-your-own-machine).

## Startup

In **Settings > Startup**, enable **Launch OwlWhisp when I log in** to start with
the system. Starting with 0.1.3, **Start minimized to tray** keeps the main window
hidden on launch. Click **Save** and restart to apply it. Open the window from the
tray menu; if no system tray is available, the window opens normally.

## Background memory

The client and the speech model have different memory costs. OwlWhisp loads the model on demand;
version 0.1.1 unloads it after **5 minutes idle** by default. In **Settings → Background
memory**, choose 1, 5, 15 or 30 minutes, or **Keep model loaded**. Explicit GPU selections run in a
separate worker, so unloading also releases its provider libraries and driver allocations.

After restarting the updated **full desktop client**, OwlWhisp used **43 MiB private resident RAM**
and **88 MiB total working set** with the model unloaded. A simultaneous 60-second Windows x64
snapshot on 2026-10-03 measured the following medians, including every child process:

| Client | Private resident RAM, MiB | Total working set, MiB | Observed state |
|---|---:|---:|---|
| **OwlWhisp, updated source build** | **43** | **88** | Model unloaded |
| OpenWhispr 1.10.1 | 1,000 | 1,513 | Parakeet 0.6B v3 INT8 CPU server running |
| Superwhisper 1.6.5 | 685 | 1,558 | Cohere Transcribe Q4 selected; model residency unverified |

This is a snapshot of the running configurations, with different model states, rather than a
matched-model benchmark. Total working sets can count shared pages more than once; private
resident RAM excludes those pages. A loaded Parakeet model can use hundreds of MiB to several GiB,
depending on the backend and TensorRT cache. Reloading measured about 2.6 seconds on CPU and 4.3
seconds with a prepared TensorRT cache on this machine.

See [memory measurements and client comparison](docs/memory.md) for the recorded samples,
conditions, startup tradeoff and repeatable measurement script. The idle-memory controls are
available starting with v0.1.1.

## Supported platforms

All packages are native **64-bit** builds. Choose the architecture of your computer.

| Platform | Package | Included inference | Desktop support |
|---|---|---|---|
| **Windows x64**, Intel/AMD, Windows 10/11 | NSIS installer or portable ZIP | CPU, DirectML, WebGPU; CUDA/TensorRT can be installed in Settings | Primary; verified on RTX 4080 Laptop |
| **Windows ARM64**, Windows 11, including Snapdragon X/X2 | NSIS installer or portable ZIP | CPU, WebGPU; QNN NPU on compatible Snapdragon hardware after Prepare NPU model | Primary; verified on Snapdragon X2 |
| **macOS Apple Silicon**, ARM64, macOS 13.3+ | `.app` in a ZIP | CPU and bundled WebGPU/Metal provider | Preview |
| **macOS Intel**, x64, macOS 13.3+ | `.app` in a ZIP | CPU; pinned ONNX Runtime built from source for Intel, reused with hash verification | Preview |
| **Linux x64**, built on Ubuntu 22.04 | Portable `.tar.gz` | CPU and bundled WebGPU/Vulkan provider | Preview |
| **Linux ARM64**, built on Ubuntu 24.04 | Portable `.tar.gz` | Parakeet on CPU or CUDA 13 (system GPU libraries required); sherpa models unavailable | Preview |

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
The current **0.1.8** release is a prerelease; Windows installers and macOS bundles are unsigned.

| Platform | Portable archive | Installer |
|---|---|---|
| Windows x64 | [ZIP](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.8/OwlWhisp-0.1.8-x86_64-pc-windows-msvc.zip) | [Setup](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.8/OwlWhisp-0.1.8-x86_64-pc-windows-msvc-setup.exe) |
| Windows ARM64 | [ZIP](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.8/OwlWhisp-0.1.8-aarch64-pc-windows-msvc.zip) | [Setup](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.8/OwlWhisp-0.1.8-aarch64-pc-windows-msvc-setup.exe) |
| macOS Apple Silicon | [ZIP](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.8/OwlWhisp-0.1.8-osx-arm64-macos.zip) | — |
| macOS Intel | [ZIP](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.8/OwlWhisp-0.1.8-osx-x64-macos.zip) | — |
| Linux x64 | [tar.gz](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.8/OwlWhisp-0.1.8-linux-x64.tar.gz) | — |
| Linux ARM64 | [tar.gz](https://github.com/Fil-Shtopor/OwlWhisp/releases/download/v0.1.8/OwlWhisp-0.1.8-linux-arm64.tar.gz) | — |

Starting with 0.1.2, **Settings > Application updates** checks GitHub at startup and once a day,
with controls for automatic checks and preview releases. Windows x64/ARM64 packages can download
and verify the matching installer, then open it from the app. Install 0.1.2 manually once to get
this mechanism. [Windows signing setup](docs/windows-signing.md) is ready for an existing trusted
certificate or cloud signing profile; signing is not enabled yet.

Each platform has a `SHA256SUMS-*.txt` file. Release builds verify executable/runtime architecture,
model manifests and licence files, then unpack and load the packaged runtime on the native runner.
Publication requires all six platforms and all checksums. After publication, CI downloads every asset
without authentication and checks its SHA-256. Models are downloaded separately on first use.

## Hardware tests

45 simulated configurations cover Windows/macOS/Linux, x64/ARM64, CPU/GPU/NPU hardware,
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
