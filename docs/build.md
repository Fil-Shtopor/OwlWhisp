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

# Everything (CLI + the desktop app):
pwsh -File scripts\build\build-windows-arm64.ps1 -Release
```

That builds; it does not package. See [Packaging](#packaging) below.

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
# One command: builds the app, stages the runtime, links a model, and launches.
pwsh -File scripts\build\run-app.ps1 -ModelDir "C:\path\to\parakeet-tdt-0.6b-v3"
```

Or plainly:

```bash
cargo build -p lw-gui --release          # target/release/localwisper-gui.exe
```

No feature flag is needed to get a usable window, and there is no dev mode distinct from a build:
`cargo run -p lw-gui` is the whole story. A release build sets `windows_subsystem = "windows"`, so
it opens no console -- run the debug build, or set `LW_LOG=debug`, when you want to watch it think.
Logs go to `%APPDATA%\ai.localwisper.app\logs\` either way.

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
workflow against `localwisper-gui.exe`, so it cannot lapse quietly.

## Packaging

```powershell
pwsh -File scripts\build\package-windows.ps1              # after building
```

It stages the executable, `runtime\win-arm64\` and the licence notices into
`dist\LocalWisper-<version>-<target>\`, zips that, and -- if `makensis` is on PATH -- builds
`dist\LocalWisper-<version>-<target>-setup.exe` from `scripts\build\localwisper.nsi`. Without
NSIS it says so and stops after the zip, which is a complete, runnable build on its own.

The installer is per-user (`%LOCALAPPDATA%\Programs\LocalWisper`), because this application uses
no privilege it cannot get as the user: the hotkey is a session keyboard hook, autostart is HKCU,
and the models live under `%APPDATA%`. Its uninstaller removes the autostart registration but
leaves settings, models and logs alone, and says where they are -- a model set is tens of
gigabytes and an hour of downloading.

**Not yet done:** there is no macOS or Linux packager. Those release jobs publish the bare
executable, which is a build artefact rather than something to hand a user. `cargo-packager` would
cover all three but does not build here: a dependency of a dependency needs clang, the same clang
the `sherpa` engine needs.
