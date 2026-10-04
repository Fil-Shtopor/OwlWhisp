# OwlWhisp — Build & Run

## Build matrix

| Target | Package/profile | Status |
|---|---|---|
| Windows ARM64 (`aarch64-pc-windows-msvc`) | NSIS + ZIP; CPU, WebGPU, QNN on compatible Snapdragon | Primary; NPU/CPU verified on X2 |
| Windows x64 (`x86_64-pc-windows-msvc`) | NSIS + ZIP; CPU, DirectML, WebGPU; optional CUDA/TensorRT | Primary; verified on RTX 4080 Laptop |
| macOS ARM64 (`aarch64-apple-darwin`) | `.app` ZIP; CPU + WebGPU, macOS 13.3+ | Preview; desktop integration is partial |
| macOS x64 (`x86_64-apple-darwin`) | `.app` ZIP; CPU, macOS 13.3+ | Preview; source-built ORT 1.28.1, reused from a hash-pinned verified release |
| Linux x64 (`x86_64-unknown-linux-gnu`) | Tarball; CPU + WebGPU, built on Ubuntu 22.04 | Preview; desktop integration is partial |
| Linux ARM64 (`aarch64-unknown-linux-gnu`) | Tarball; Parakeet CPU + CUDA 13 provider, built on Ubuntu 24.04 | Preview; no pinned no-TTS sherpa package available |

CI checks and tests all six native targets. Release builds unpack each archive on its native
runner and load the packaged runtime before publishing. macOS/Linux hotkeys, text injection and
foreground-app detection remain unimplemented; see [platform support](../README.md#supported-platforms).

## Prerequisites (Windows ARM64)

- **Rust** stable with the `aarch64-pc-windows-msvc` host toolchain (`rustup default stable`).
- **MSVC ARM64 toolset + Windows SDK.** Either install Visual Studio Build Tools with the
  "MSVC v14x — ARM64/ARM64EC build tools" and a Windows 11 SDK component, **or** use a portable
  toolset and source its env script before building (this repo's dev machine uses
  `~/.msvc-arm64/env-arm64.{ps1,sh}`).

There is no Node, no npm and no WebView2 requirement any more: the window is Rust, drawn on a CPU
rasteriser. **NSIS** (optional) is needed only to build the installer; without it the packaging
script still produces the portable folder and the zip.

TLS note: the workspace deliberately uses `reqwest` with **native-TLS** (SChannel), not
`rustls`/`aws-lc-rs` — the latter fails to assemble its ARM64 assembly under the portable MSVC
toolset. Do not add rustls-based HTTP deps.

## One-time: stage the ONNX Runtime + QNN DLLs

```powershell
pwsh -File scripts\runtime\fetch-runtime.ps1        # -> runtime\win-arm64\*.dll (native ARM64)
```

This downloads the official ARM64 ONNX Runtime and the native-ARM64 Qualcomm QNN execution-provider
DLLs (from NuGet `Qualcomm.ML.OnnxRuntime.QNN`) plus their licences. All DLLs must be machine
`0xAA64`. These are **redistributable** (MIT for ORT + the EP; the Qualcomm QNN libs under the
Qualcomm AI Stack License, object-code-only, shipped with `Qualcomm_LICENSE.pdf`).

## One-time: fetch a model

```bash
python scripts/models/download_model.py models/manifests/parakeet-tdt-0.6b-v3.json \
    --dest "$LOCALAPPDATA/OwlWhisp/models" --target cpu_int8
```

(The shipping app downloads models itself with the same SHA-256 verification; this is a dev
convenience.) The NPU target additionally needs a static-shape encoder; see
[`x2-npu.md`](x2-npu.md) for how it is produced/cached.

## Build

```bash
# CLI + library (native ARM64). On the portable-toolset dev machine:
source ~/.msvc-arm64/env-arm64.sh
cargo build --release -p lw-cli --target aarch64-pc-windows-msvc

# Everything (CLI + the desktop app):
pwsh -File scripts\build\build-windows-arm64.ps1 -Release
```

That builds; it does not package. See [Packaging](#packaging) below.

## Run (CLI)

```bash
LW=target/release/lw.exe
RTD=runtime/win-arm64
MODEL="$LOCALAPPDATA/OwlWhisp/models/parakeet-tdt-0.6b-v3"

# Hardware / runtime report (shows the real provider + NPU status)
"$LW" --runtime-dir "$RTD" diagnose

# Transcribe a WAV (CPU or NPU; NPU prepares+caches the HTP context on first run)
"$LW" --runtime-dir "$RTD" transcribe clip.wav --model-dir "$MODEL" --backend auto

# Benchmark over the test fixtures
"$LW" --runtime-dir "$RTD" bench tests/fixtures/audio --model-dir "$MODEL" --backend cpu
```

`--backend auto` uses the NPU when available and falls back to CPU, always reporting which it used.

## Run (desktop app)

```powershell
# One command: builds the app, stages the runtime, links a model, and launches.
pwsh -File scripts\build\run-app.ps1 -ModelDir "C:\path\to\parakeet-tdt-0.6b-v3"
```

Or plainly:

```bash
cargo build -p lw-gui --release          # target/release/owlwhisp.exe
```

No feature flag is needed to get a usable window, and there is no dev mode distinct from a build:
`cargo run -p lw-gui` is the whole story. A release build sets `windows_subsystem = "windows"`, so
it opens no console -- run the debug build, or set `LW_LOG=debug`, when you want to watch it think.
Logs go to `%APPDATA%\ai.owlwhisp.app\logs\` either way.

At runtime the app locates its pieces as follows (the launcher script wires all three up):

| Piece | Where it is looked for |
|---|---|
| ONNX Runtime + QNN DLLs | `LW_RUNTIME_DIR`, else `runtime/<platform>/` or `runtime/` beside the executable |
| DirectML runtime (Windows x64) | `runtime/win-x64-directml/` beside the CUDA runtime; loaded by the app's persistent DirectML worker |
| Model | `%APPDATA%\ai.owlwhisp.app\models\<model_id>` (`model_id` comes from settings) |
| QNN context cache | `%APPDATA%\ai.owlwhisp.app\cache` |

Hold **Ctrl+Alt+Space** to dictate; the tray icon opens Settings / Diagnostics / Quit.

On Windows x64, `fetch-runtime.ps1` also stages DirectML and TensorRT. DirectML uses a separate
Windows ML ONNX Runtime 1.28 core because the CUDA core cannot be replaced after the app starts.
TensorRT is staged with the Ada SM 8.9 builder resource used by the RTX 4080 test machine.
Parakeet now uses FP16 and an explicit 1/600/2000-frame profile instead of rebuilding for each
new clip length. On 2026-10-03, fully prepared real-speech runs measured RTF 0.0077–0.0097,
against CUDA 0.0246–0.0275, with identical WER 5.4%. First preparation took about 136 seconds;
the engine cache used 1.25 GB and subsequent session loading took about 4 seconds. TensorRT is
an optional prepared mode; CUDA precedes it in the ordinary GPU startup policy. See
[the profile comparison](accelerator-profiles-2026-10-03.md) for cold/new-shape timings,
scope and reproduction commands. The two runtimes
and their licence notices are included by `package-windows.ps1`.

## Tests

```bash
source ~/.msvc-arm64/env-arm64.sh
cargo test --workspace            # unit tests (no model/NPU needed)
cargo test --workspace -- --ignored   # integration tests that need the model + runtime present
```

## Other platforms

- **macOS**: `cargo build --release --target aarch64-apple-darwin` (needs Xcode CLT). The audio,
  clipboard and (via ORT CoreML EP) inference compile; hotkeys/overlay/injection use the macOS
  platform module. ANE acceleration is not verified on our hardware — see `x2-npu.md` §macOS.
- **Linux**: `cargo build --release`; the platform layer is scaffolding (audio via cpal compiles;
  hotkeys/injection are stubs returning `Unavailable`). ORT CPU EP works.

## The `sherpa` engine (Whisper, Moonshine, SenseVoice, …)

**The shipped Windows build includes it**, because fourteen of the fifteen catalog entries need it
and a model that downloads and verifies but cannot be loaded is not a feature.
`scripts\build\build-windows-arm64.ps1` stages the libraries and passes `--features sherpa` to both
the CLI and the desktop app; `-NoSherpa` builds the Parakeet-only binary.

It is **not** a default *cargo* feature, and that is deliberate. Enabling it needs one extra step,
and that step is **not optional** — building it the obvious way lets `sherpa-onnx-sys` fetch its
stock archive, which links GPL-3.0 espeak-ng, which cannot ship alongside the proprietary Qualcomm
runtime. A default feature would make the wrong build the easy one. See
[`licenses.md`](licenses.md) for the full reasoning.

One more prerequisite: **clang on PATH**. `sherpa-onnx-sys`'s build script pulls `ureq → rustls →
ring` to download the archive, and `ring` has no aarch64-windows assembly path MSVC alone can
build. The build script checks for it up front and names this feature in the error, rather than
letting cc-rs fail inside a dependency nobody asked for.

```powershell
# Fetch the no-tts archive and generate the stub libraries the crate's link flags demand.
$env:SHERPA_ONNX_LIB_DIR = (pwsh -File scripts\build\fetch-sherpa.ps1 -Quiet)
cargo build --release -p lw-cli --features sherpa
```

Bash:

```bash
export SHERPA_ONNX_LIB_DIR="$(powershell -NoProfile -File scripts/build/fetch-sherpa.ps1 -Quiet)"
cargo build --release -p lw-cli --features sherpa
```

The script is idempotent: it re-uses an already-staged archive and only downloads once. It also
refuses to continue if the archive it fetched contains a real espeak-ng, so the licence guarantee
cannot silently lapse when upstream changes.

**Verified** on Windows ARM64 with sherpa-onnx 1.13.6: the build links, all 44
`lw-engine-sherpa` tests pass, and the application builds with the feature -- including
`a_sherpa_build_can_actually_run_the_gated_models`, which asserts that the feature reaches the
engine crate *and* that the catalog then stops reporting a sherpa blocker on a gated entry. The
linked binary was searched for the strings the licence argument turns on: 390 hits for
`sherpa-onnx`, **zero** for `espeak` and `piper_phonemize`. That check now runs in the release
workflow against `owlwhisp.exe`, so it cannot lapse quietly.

## Packaging

```powershell
# Windows
pwsh -File scripts\build\package-windows.ps1 -Target aarch64-pc-windows-msvc -Platform win-arm64

# macOS or Linux (run on the matching host)
pwsh -File scripts/build/package-unix.ps1 -Target aarch64-apple-darwin -Platform osx-arm64
pwsh -File scripts/build/package-unix.ps1 -Target x86_64-unknown-linux-gnu -Platform linux-x64
```

Windows packages contain a portable ZIP and an NSIS per-user installer. macOS packages contain a
self-contained `.app` bundle in a ZIP; Linux packages are portable `.tar.gz` archives. All stage
the matching ONNX Runtime files and licence notices. Pushing a `v*` tag runs the same matrix on
GitHub Actions and attaches each package plus SHA-256 checksums to the GitHub Release.

## Hardware-selected runtime add-ons (Windows x64)

The standard Windows x64 package includes CPU, DirectML and WebGPU. CUDA/cuDNN and TensorRT
are optional: Settings ? Accelerator ? **Download runtime** installs the pinned packages for
this machine in `%APPDATA%/ai.owlwhisp.app/runtimes/win-x64`. No administrator rights or full CUDA
Toolkit installation are required. A missing/old NVIDIA display driver has a separate **Install
driver** action. CUDA 13 requires a driver exposing CUDA API 13.0 or later and compute capability
7.5 or later; older cards can use compatible DirectML/WebGPU providers.

TensorRT 10.14.1.48 downloads only the matching Windows builder partition (SM 75, 80, 86, 89,
90 or 120). Detection uses the NVIDIA driver API, never a table of marketing names. The add-on
lock pins archive sizes and SHA-256/SHA-512 hashes, allowlists extracted files, and carries the
redistribution licences. Verified bundled files are reused. Interrupted downloads support HTTP
Range resumption. `scripts/runtime/lock-addons.py` deliberately refreshes the lock only when a
maintainer runs it; the shipping app never selects a newer package from a live feed.

Installation probes the provider in a fresh process, then prepares/checks the GPU model before
selecting the backend. Optional runtimes run in persistent workers because ONNX Runtime's API is
process-global. Installing another runtime never replaces loaded DLLs or requires restarting the
GUI. Failed downloads leave the current working backend intact and expose a retry action.

TensorRT caches are separated by GPU UUID/capability, driver compatibility, operating system,
runtime binary fingerprints, model graph hash/external-weight fingerprint, precision, shapes and
diagnostic overrides. Changing these creates a new cache; old unlabelled caches are not imported.
The first build after this change therefore prepares a fresh engine once.

```powershell
# Default portable package: CPU + DirectML + WebGPU, NVIDIA add-ons installed in Settings
pwsh -File scripts/runtime/fetch-runtime.ps1 -Platform win-x64 -BundleProfile Base
pwsh -File scripts/build/package-windows.ps1 -Target x86_64-pc-windows-msvc -NoInstaller

# Optional offline NVIDIA bundle for an explicitly selected architecture
pwsh -File scripts/runtime/fetch-runtime.ps1 -Platform win-x64 -BundleProfile Full -TensorRtSm 89
pwsh -File scripts/build/package-windows.ps1 -Target x86_64-pc-windows-msvc -WithNvidiaRuntime -NoInstaller
```

Automatic package downloads currently target **Windows x64**. Windows ARM64 retains bundled QNN;
macOS/Linux retain their existing bundled providers. No automatic CUDA/TensorRT installer is
advertised on those platforms. Selection rules cover other NVIDIA architectures, but inference
and performance have been measured only on the RTX 4080 Laptop; other cards need hardware QA.

See [Simulated hardware tests](hardware-tests.md) for the OS/architecture/GPU detection and
installation-policy matrix, which runs without native providers or downloaded models.

## NVIDIA RTX Spark (Windows ARM64)

Use the **Windows ARM64** installer/ZIP. From 0.1.5, it bundles a native DirectML runtime in
`runtime/win-arm64-directml/` as well as the WebGPU provider in `runtime/win-arm64/`. Each provider
runs in a separate worker, so its ONNX Runtime core cannot conflict with QNN or the main process.
Install the NVIDIA Windows ARM64 driver, choose Parakeet TDT 0.6B v3, and use **GPU (DirectML)**
or **GPU (WebGPU)**. Complete the GPU encoder setup in Settings and run Benchmark to compare
performance. **Experimental: not tested on a physical RTX Spark.** Native package checks and
simulated NVIDIA ARM64 hardware tests do not replace that validation.

CUDA/classic TensorRT runtime downloads are Windows x64 only; x64 DLLs cannot be loaded by the
ARM64 application. RTX Spark and DGX Spark are separate platform targets.

## NVIDIA DGX Spark (Linux ARM64)

Use the **Linux ARM64** release archive on Spark's native DGX OS desktop. From 0.1.4 it contains
ONNX Runtime 1.30.0's matched ARM64 C library and CUDA 13 provider. Only the native libraries and
MIT/third-party notices are extracted from Microsoft's pinned official GPU wheel; Python is a
build-time tool and is not needed to run the app. The archive is size/SHA-256 verified before
staging. The CUDA library includes SM 12.1 kernels for GB10.

The GPU path requires the system NVIDIA driver and **ARM64 CUDA 13** libraries (cuBLAS, cuRAND,
CUDA runtime, NVRTC and cuFFT), plus **cuDNN 9 for CUDA 13** and zlib. Keep the compatible NVIDIA
stack supplied with DGX OS. `nvidia-smi` must report the GB10 GPU; its CUDA version is the driver's
maximum supported API, so that field alone does not prove the runtime libraries are installed.
See [NVIDIA's Spark software versions](https://docs.nvidia.com/dgx/dgx-spark/release-notes.html),
[ONNX Runtime's CUDA requirements](https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html)
and [NVIDIA's cuDNN installation guide](https://docs.nvidia.com/deeplearning/cudnn/installation/latest/linux.html).

If cuDNN is missing, with the matching NVIDIA Ubuntu ARM64/SBSA repository already configured:

```sh
sudo apt-get update
sudo apt-get install cudnn9-cuda-13 zlib1g
```

CUDA/cuDNN libraries must be discoverable by the system loader. A standard DGX OS installation
should register its library directories; for a custom CUDA installation, add its actual library
directory to `LD_LIBRARY_PATH` **before launching** OwlWhisp, for example:

```sh
LD_LIBRARY_PATH="/usr/local/cuda-13.0/lib64${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ./owlwhisp
```

In **Settings > Accelerator**, select **NVIDIA GPU (CUDA)** for Parakeet, download its GPU model
when prompted and run **Benchmark > Compare all accelerators**. Automatic selection can prefer
CPU; explicitly select CUDA to evaluate the GPU. **Diagnostics** reports hardware and provider
usability separately. A missing dependency or failed model check does not qualify as GPU support.

For a source build, staging chooses the same pinned runtime automatically:

```sh
pwsh -File scripts/runtime/fetch-runtime.ps1 -Platform linux-arm64
cargo build --locked --release -p lw-gui
```

`-SkipCuda` stages the CPU-only runtime for a local development build instead. Runtime library
architecture and packaged CPU startup are verified on native ARM64 CI; GB10 detection is also
covered with and without a display adapter. **Physical Spark CUDA inference is still untested.**
The official ARM64 GPU wheel does not include TensorRT, so that accelerator is not bundled or
claimed for Spark. Other Linux ARM devices need their own compatible CUDA 13 stack; this is not a
promise of compatibility with older JetPack installations. Linux global hotkeys, text injection
and foreground-app detection remain unimplemented.
