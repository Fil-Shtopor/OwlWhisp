# LocalWisper — Build & Run

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
- **Node.js 18+** and **npm** (for the Tauri frontend). `@tauri-apps/cli` has a native win32-arm64
  binary.
- **WebView2 runtime** (present on Windows 11 by default).

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
    --dest "$LOCALAPPDATA/LocalWisper/models" --target cpu_int8
```

(The shipping app downloads models itself with the same SHA-256 verification; this is a dev
convenience.) The NPU target additionally needs a static-shape encoder; see
[`x2-npu.md`](x2-npu.md) for how it is produced/cached.

## Build

```bash
# CLI + library (native ARM64). On the portable-toolset dev machine:
source ~/.msvc-arm64/env-arm64.sh
cargo build --release -p lw-cli --target aarch64-pc-windows-msvc

# Everything (CLI + Tauri app + frontend):
pwsh -File scripts\build\build-windows-arm64.ps1 -Release
```

The Tauri NSIS installer lands under
`target/aarch64-pc-windows-msvc/release/bundle/nsis/`.

## Run (CLI)

```bash
LW=target/release/lw.exe
RTD=runtime/win-arm64
MODEL="$LOCALAPPDATA/LocalWisper/models/parakeet-tdt-0.6b-v3"

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
# One command: builds the frontend + app, stages the runtime, links a model, and launches.
pwsh -File scripts\build\run-app.ps1 -ModelDir "C:\path\to\parakeet-tdt-0.6b-v3"
```

Or step by step:

```bash
cd app/frontend && npm install && npm run build && cd ../..
pwsh -File scripts\build\build-windows-arm64.ps1 -App     # production build + NSIS installer
npx @tauri-apps/cli@2 dev                                 # dev mode (hot-reload UI)
```

> **Important:** building the app with plain cargo instead of the Tauri CLI requires the production
> feature, otherwise Tauri loads `build.devUrl` and the window shows *"localhost refused to
> connect"*:
>
> ```bash
> cargo build -p localwisper --release --features custom-protocol
> ```
>
> `tauri build` sets this feature for you; `tauri dev` deliberately leaves it off.

At runtime the app locates its pieces as follows (the launcher script wires all three up):

| Piece | Where it is looked for |
|---|---|
| ONNX Runtime + QNN DLLs | `LW_RUNTIME_DIR`, else `runtime/<platform>/` or `runtime/` beside the executable |
| Model | `%APPDATA%\ai.localwisper.app\models\<model_id>` (`model_id` comes from settings) |
| QNN context cache | `%APPDATA%\ai.localwisper.app\cache` |

Hold **Ctrl+Alt+Space** to dictate; the tray icon opens Settings / Diagnostics / Quit.

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
the CLI and the Tauri app; `-NoSherpa` builds the Parakeet-only binary.

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
`lw-engine-sherpa` tests pass, and the Tauri app builds with the feature (14 tests in the app
crate, including `a_sherpa_build_can_actually_run_the_gated_models`, which asserts that the
feature reaches the engine crate *and* that the catalog then stops reporting a sherpa blocker on a
gated entry). The linked `localwisper.exe` was searched for the strings the licence argument turns
on: 390 hits for `sherpa-onnx`, **zero** for `espeak` and `piper_phonemize`.

## Packaging

`cargo tauri build` produces the native installers for the host platform (`bundle.targets` is
`all`): `.exe`/`.msi` on Windows, `.dmg`/`.app` on macOS, `.deb`/`.rpm`/`.AppImage` on Linux.
`build.rs` copies `runtime/<platform>/` into the bundle first, so an installed application has its
ONNX Runtime and execution providers beside it.

Note that `targets: "all"` on Windows builds both NSIS and WiX/MSI, and the Tauri CLI downloads
each toolchain on first use. To build just one: `cargo tauri build --bundles nsis`.
