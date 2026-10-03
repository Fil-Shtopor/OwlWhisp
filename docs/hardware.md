# OwlWhisp — Platforms and accelerators

_What runs where, what is verified on real hardware, and exactly what each remaining path would
take. Updated 2026-09-16._

The rule this project follows: **a backend is only claimed once it has been observed executing.**
Everything below is labelled accordingly.

| Legend | Meaning |
|---|---|
| ✅ **verified** | Executed and measured on that hardware, producing correct transcripts |
| ⚙️ **implemented, untested** | The code path exists and compiles; nobody has run it on that chip |
| 📦 **needs a redistributable** | Implemented; needs a provider library we do not ship (vendor SDK) |
| 🧩 **needs work** | Not implemented |

---

## 1. How acceleration works here

Every accelerator except the CPU reaches ONNX Runtime through its **plugin execution-provider**
mechanism: a vendor ships a provider library, OwlWhisp registers it by name at runtime
(`RegisterExecutionProviderLibrary`), asks it for devices, and pins the session to those devices.
That is the same mechanism the verified Qualcomm NPU path uses — so adding a vendor is *adding a
library to the runtime directory*, not rebuilding ONNX Runtime.

One distinction governs everything below:

- **A GPU consumes the ordinary ONNX graph.** One artifact serves NVIDIA, AMD, Intel, Apple and
  Qualcomm GPUs. This is why GPU coverage generalizes.
- **An NPU wants the graph quantized and compiled for that specific silicon.** That artifact has to
  be produced *and validated* on that hardware. This is why NPU coverage does not generalize, and
  why "we support every NPU" is not something this project can honestly claim by writing code.

`Accelerator::needs_dedicated_artifact` encodes exactly that line.

---

## 2. Measured on the target machine

Snapdragon X2 Elite Extreme (X2E94100), Windows 11 ARM64, 12 FLEURS clips (en/ru/es/uk), via
`lw bench tests/fixtures/audio --backend <x>`:

| Backend | Provider | mean RTF | word-weighted WER | Status |
|---|---|---:|---:|---|
| **NPU** | QNN / Hexagon HTP V81, fp16 | **0.0160** | 4.8 % | ✅ verified |
| **CPU** | ONNX Runtime CPU EP, int8 | 0.0324 | 5.4 % | ✅ verified |
| **GPU** | WebGPU (Dawn → D3D12) on Adreno, int8 | 0.0862 | 4.8 % | ✅ verified |

All three produce correct sentences in all four languages.

The GPU row is the **shipped artifact set** — the same quantized encoder the CPU uses, run on the
GPU. That matters: GPU acceleration needs no extra download and works on a standard install. With
a locally produced static fp32 encoder the same GPU measures 0.0609 / 5.4 % instead; the engine
prefers a dynamic fp32 encoder, then a static one, then the quantized one, and reports which it
used.

The ordering is specific to this machine: an 18-core Oryon CPU is genuinely faster than its
integrated mobile GPU for this model. On a desktop with a discrete GPU the GPU row is expected to
move well above the CPU — but that is an expectation, not a measurement, and no such machine was
available.

---

## 3. Platform support

| Platform | Package | Included inference | Desktop status |
|---|---|---|---|
| Windows 11 ARM64 | NSIS installer, portable ZIP | CPU, WebGPU, QNN on compatible Snapdragon with a matching model | Primary; X2 verified |
| Windows 10/11 x64 | NSIS installer, portable ZIP | CPU, DirectML, WebGPU; CUDA/TensorRT installed in Settings | Primary; RTX 4080 Laptop verified |
| macOS 13.3+ Apple Silicon | `.app` ZIP | CPU, bundled WebGPU/Metal | Preview |
| macOS 13.3+ Intel | `.app` ZIP | CPU; ONNX Runtime built from pinned 1.28.1 source | Preview |
| Linux x64 (Ubuntu 22.04 build) | Portable tarball | CPU, bundled WebGPU/Vulkan | Preview |
| Linux ARM64 (Ubuntu 24.04 build) | Portable tarball | Parakeet CPU only; sherpa omitted | Preview |

macOS/Linux global hotkeys, automatic text injection and foreground-app detection are still
unimplemented. CoreML/ANE and Intel/AMD NPU model packages are not shipped. Linux ARM64 lacks a
no-TTS sherpa archive for the pinned version, so the other CPU models remain unavailable there.

All six native targets have CI build/test jobs. Release archives are unpacked on native runners,
checked for the right architecture, model manifests and licence notices, then exercised with the
packaged CPU runtime. This validates packaging and startup; it does not claim GPU/NPU inference
or a complete desktop workflow was tested on those virtual runners. Windows GPU inference was
measured on the RTX 4080 Laptop, and Qualcomm NPU inference on the Snapdragon X2.

For download links and current requirements, see [README](../README.md#supported-platforms).
Windows and macOS packages are unsigned.

---

## 4. Accelerator status

`lw diagnose` and the app's Settings/Diagnostics report hardware, provider presence, registration
and device count separately. **Usability requires a present, registered provider, a matching device
and no probe error.** A model must also support that provider before it can be selected.

| Accelerator | Standard package | Additional requirements |
|---|---|---|
| CPU | All six platforms | Model download; Linux ARM64 supports Parakeet only |
| Qualcomm NPU / QNN | Windows ARM64 | Compatible Snapdragon hardware, driver and matching HTP model artifact |
| WebGPU | Windows x64/ARM64, macOS ARM64, Linux x64 | Compatible GPU/driver; Linux needs a Vulkan loader |
| NVIDIA CUDA | Optional Windows x64 download in Settings | Compatible NVIDIA driver; supported Parakeet model |
| NVIDIA TensorRT | Optional Windows x64 download in Settings | Driver and supported compute capability; first run builds an engine cache |
| DirectML | Windows x64, in a separate bundled runtime | D3D12 GPU/driver; supported Parakeet model |
| Apple CoreML / ANE | Not included | Provider and a validated model export are still needed |
| Intel OpenVINO | Not included | Compatible provider SDK; a dedicated artifact for an NPU |
| AMD Vitis AI | Not included | Compatible Ryzen AI SDK and a dedicated NPU artifact |

A provider library on disk alone does not make an accelerator available. The three model statuses
are **Available**, **Need additional action** (with an executable setup action) and **Unavailable**.
Hardware detection and library-selection policy have simulated tests; GPU/NPU performance needs
real hardware measurements.

---

## 5. Runtime installation

The runtime directory is `runtime/<platform>/` in the repo and in the portable Windows/macOS
layout. Linux places `runtime/` beside the executable. `LW_RUNTIME_DIR` overrides discovery.
`scripts/runtime/fetch-runtime.ps1` stages the platform's base runtime and bundled providers.
The Intel macOS runtime is built from pinned ONNX Runtime 1.28.1 source.

Windows x64 standard packages omit the large NVIDIA dependencies. **Settings > Models > Download
runtime** installs pinned CUDA/TensorRT packages selected for the detected compute capability,
then probes them in a separate worker. No full CUDA Toolkit is needed. A required driver update
is offered separately. The bundled DirectML runtime is probed in its own worker as well, so its
ORT version cannot conflict with the main runtime.

See [hardware-selected runtime add-ons](build.md#hardware-selected-runtime-add-ons-windows-x64)
for the package catalogue and supported NVIDIA architectures. Automatic NVIDIA package installation
is currently Windows x64 only; Linux NVIDIA support requires a separately staged compatible runtime.

OpenVINO, Vitis AI and CoreML are developer integrations without automatic package installation.
Their libraries and model artifacts must be supplied and tested on the corresponding hardware.
Intel/AMD NPU inference is not part of the standard release.

A forced backend fails when it is unusable. Run `lw diagnose` and a model benchmark to check the
actual provider; registration alone is not evidence of accelerated inference.

---

## 6. Apple platforms

Both Apple Silicon and Intel builds are packaged as an `.app` inside a ZIP. Both include CPU;
Apple Silicon also includes the WebGPU/Metal provider. CoreML/ANE inference is not shipped.

Native macOS CI checks the workspace, simulated configurations and loading the packaged CPU
runtime. It does not measure Apple GPU/ANE inference. Global hotkeys, text injection,
foreground-app detection and a native non-activating overlay still need platform implementation.

---

## 7. Linux

Linux x64 and ARM64 builds are portable `.tar.gz` archives, with GTK 3 and ALSA system libraries
required. The GUI supports X11 and Wayland. x64 includes CPU and WebGPU (which needs a compatible
Vulkan loader/driver); ARM64 currently includes Parakeet CPU only because the pinned sherpa release
has no no-TTS ARM64 Linux archive.

Native CI checks both architectures and the packaged CPU runtime. Global hotkeys, text injection
and foreground-app detection are unimplemented. Wayland integration will require suitable desktop
portals or another supported input mechanism. Tray behavior depends on the desktop environment.

---

## 8. Choosing and seeing the backend

- **Settings → Backend** lists Automatic, Any NPU, Any GPU, CPU, and one entry per provider.
  Providers this machine cannot use are shown with the reason.
- **Automatic** tries every usable accelerator best-first — **NPU → CPU → GPU** — and falls back.
  The CPU sitting ahead of the GPU is deliberate: the only GPU measurement this project has shows
  an integrated GPU losing to a strong CPU (0.0862 vs 0.0324 on the X2). A discrete GPU would very
  likely win, but that is an expectation, and defaulting to a path 2.7× slower on the one machine
  we can check is not worth shipping. Pick **Any GPU** (or a specific provider) to use it, and use
  the benchmark to find out which is faster on your machine.
  Every other choice is strict: it fails rather than silently running elsewhere.
- The engine reports the provider that **actually executed**, not the one requested, and its
  backend-selection notes say why it chose or fell back. Those appear in the benchmark report and
  in Diagnostics.
- From a terminal: `lw diagnose` for the table, `lw bench --quick --backend <x>` to measure one.

---

## 9. Adding a new accelerator

1. Add a variant to `lw_core::capabilities::Accelerator` with its EP registration name and library
   file name. Selection, settings, diagnostics, the CLI and the benchmark pick it up from there.
2. Stage its provider library in `runtime/<platform>/`.
3. If it is an NPU, produce and validate the quantized static-shape encoder artifact on that
   hardware, and add it to the model manifest.
4. Measure it with `lw bench` and record the numbers with the machine they came from.

Steps 1–2 are an afternoon. Step 3 is the work that actually gates a new NPU.
