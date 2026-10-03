# OwlWhisp — Build & Run

## Build matrix

| Target | Status | Notes |
|---|---|---|
| **Windows ARM64** (`aarch64-pc-windows-msvc`) | **first-class, verified** | Native binary; NPU (QNN HTP V81) + CPU |
| Windows x64 (`x86_64-pc-windows-msvc`) | buildable | CPU only (QNN EP path is ARM64) |
| macOS arm64 (`aarch64-apple-darwin`) | buildable; ANE path not verified on our hardware | ORT CoreML EP / CPU |
| macOS x64 | buildable where ORT provides a build | CPU |
| Linux x64 / ARM64 | scaffolding (platform layer stubbed) | ORT CPU |

At minimum, **Windows ARM64 works correctly** (CPU and NPU transcription verified — see
[`benchmarks.md`](benchmarks.md)).

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
TensorRT is staged with the Ada SM 8.9 builder resource used by the RTX 4080 test machine. Its
dynamic Parakeet graph did transcribe successfully there, but compiled for each clip length. Before
enabling its engine cache, two clips measured RTF above 11. With the cache enabled, the same six
second clip measured RTF 4.881 on its first run and 0.375 on a repeat. CUDA remains the practical
GPU choice for dictation. The TensorRT engine cache used 2.44 GB in the model directory on this
machine. The two runtimes
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
