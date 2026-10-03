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

`lw diagnose` prints this table for the machine it runs on, and the app shows the same in
Settings and Diagnostics. Present / registered / device-count are reported separately on purpose:
a driver package can be installed while the provider fails to load, and a provider can load while
finding no device. **Usability requires a present, registered provider, a matching device and no probe error.**

| Accelerator | Provider library | Ships with OwlWhisp | Needs its own model artifact | Status |
|---|---|---|---|---|
| CPU | built in | — | no | ✅ verified |
| **Qualcomm NPU** | `onnxruntime_providers_qnn.dll` | ✅ (win-arm64 only) | **yes** — HTP context per Hexagon generation | ✅ verified on V81; ⚙️ V73 (X Elite / X Plus) |
| **GPU, portable** | `onnxruntime_providers_webgpu.dll` | ✅ (win-x64, win-arm64, osx-arm64, linux-x64) | no — runs the shipped encoder | ✅ verified on Adreno; ⚙️ elsewhere |
| **NVIDIA CUDA** | `onnxruntime_providers_cuda.dll` | ✅ (win-x64 package) | no | Check with the bundled GPU runtime and a model benchmark |
| **NVIDIA TensorRT** | `onnxruntime_providers_tensorrt.dll` | ❌ | no (builds an engine cache on first run) | 📦 §5 |
| **DirectML** | `onnxruntime_providers_dml.dll` | ❌ | no | 📦 §5 |
| **Apple CoreML / ANE** | `libonnxruntime_providers_coreml.dylib` | ❌ | effectively yes (static fp16 export) | 📦 §6 |
| **Intel OpenVINO** (CPU/GPU/NPU) | `onnxruntime_providers_openvino.dll` | ❌ | **yes** for the NPU | 📦 §5 |
| **AMD Vitis AI** (Ryzen AI NPU) | `onnxruntime_providers_vitisai.dll` | ❌ | **yes** | 📦 §5 |

Everything marked 📦 is **implemented in the selection, settings, diagnostics and benchmark paths
already** — drop the provider library into the runtime directory and it appears as usable, with no
code change. What is missing is the library itself, because each is tied to a vendor SDK we cannot
redistribute and could not test.

---

## 5. Getting the vendor providers

The runtime directory is `runtime/<platform>/` in the repo, and `runtime/` beside the executable in
an installed build (`LW_RUNTIME_DIR` overrides both). `scripts/runtime/fetch-runtime.ps1` stages
ONNX Runtime, WebGPU, NVIDIA CUDA with its private CUDA 13/cuDNN 9 redistributable DLLs on win-x64,
and Qualcomm QNN on win-arm64. CUDA's DLL is the legacy provider from Microsoft's GPU NuGet: its
matching `onnxruntime.dll` is staged with it, and the app creates a CUDA session directly. A driver
or CUDA Toolkit installation alone does not add this provider to an older OwlWhisp installation.
Windows x64 standard packages omit the optional NVIDIA dependencies. Settings installs the
pinned packages for the detected compute capability and probes them in a separate worker; no
full CUDA Toolkit is needed. See [runtime add-ons](build.md#hardware-selected-runtime-add-ons-windows-x64).
For providers not shipped by the application:

| Provider | What to install | Then |
|---|---|---|
| **CUDA** | Use the win-x64 OwlWhisp package built with `fetch-runtime.ps1`; a compatible NVIDIA display driver is required | Choose **NVIDIA GPU (CUDA)** in Settings with the Parakeet model; run Benchmark to verify it |
| **TensorRT** | the above plus TensorRT 10 | copy `onnxruntime_providers_tensorrt.dll` |
| **DirectML** | Windows 10 1903+ with a D3D12 GPU | copy `onnxruntime_providers_dml.dll` + `DirectML.dll` |
| **OpenVINO** | Intel OpenVINO runtime + `Intel.ML.OnnxRuntime.EP.OpenVINO` | copy `onnxruntime_providers_openvino.dll` |
| **Vitis AI** | AMD Ryzen AI SDK | copy `onnxruntime_providers_vitisai.dll` |

Then run `lw diagnose` — it will say `present / registered / devices` for each, and
`lw bench --quick --backend cuda` (or `tensorrt`, `directml`, `openvino`, `vitisai`) measures it.
A forced backend **fails loudly** if it is not usable rather than falling back, so a number can
never be attributed to the wrong provider.

TensorRT, DirectML, OpenVINO and Vitis AI remain optional. The provider-level status does not
promise that every model can run there: sherpa models use a separate CPU-only runtime, and Parakeet
must initialize its encoder session on the selected provider before acceleration is confirmed.

**Intel and AMD NPUs specifically.** The provider integration is done, but an NPU also needs a
quantized, static-shape encoder compiled for that NPU, produced and validated on one. That is the
real gate, and it is why the Qualcomm path took a static export, an on-device compile and a 1.2 GB
cached context binary to reach 0.0160 RTF. The same work is needed per NPU vendor.

---

## 6. Apple Silicon

The macOS build compiles, bundles to `.dmg`, and the portable GPU path (WebGPU → Metal) is
available in the same way as everywhere else. What is missing:

- **CoreML EP / ANE** — the cheapest big win on a Mac. Needs a static-shape fp16 encoder export and
  `MLComputeUnits` configured. Published measurements put the Parakeet encoder at ~28 ms per 15 s
  window with 99 % of ops on the ANE, which would make it the fastest path on Apple hardware.
- The macOS platform module still needs its hotkey (CGEventTap), text injection (Cmd+V via
  CGEventPost) and non-activating NSPanel overlay.

**No Apple hardware was available**, so none of this is verified and no ANE claim is made.

---

## 7. Linux

The workspace builds, packages to `.deb` / `.rpm` / `.AppImage`, and the ORT CPU and WebGPU paths
are available (WebGPU needs a system Vulkan loader, `libvulkan.so.1`). `lw-platform`'s Linux module
is scaffolding: audio via cpal compiles, while global hotkeys, text injection and the overlay
return `Unavailable`. Wayland needs the `GlobalShortcuts` portal for hotkeys and the
`RemoteDesktop` portal (or uinput) for injection.

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
