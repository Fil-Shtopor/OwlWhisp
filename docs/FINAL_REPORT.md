# LocalWisper — Final Report

_Written 2026-08-26. This report is deliberately blunt about what is verified vs. inferred vs.
remaining. "Verified" means it was executed and observed on the target machine (Snapdragon X2 Elite
Extreme X2E94100, Windows 11 ARM64); "compiles" means it builds but was not runtime-exercised;
"stub/scaffold" means an implemented interface without a full backend._

> **Amended 2026-09-19 — the front end described below no longer exists.** Every mention of Tauri,
> React, TypeScript, WebView2 or `app/` here is history. The window is now `crates/lw-gui`: Rust,
> drawn with `iced` on a `tiny-skia` CPU rasteriser, in one process. Measured on this machine, the
> replacement costs **37 MB of private commit in one process** against the Tauri build's **240 MB
> across eight**. What was verified about the *core* — the NPU path, the error rates, the model
> catalogue — is unaffected and still stands; what this report says about the shell is superseded
> by [`architecture.md`](architecture.md) and [`build.md`](build.md).

## 0. Definition-of-Done checklist

| Item | Status |
|---|---|
| Buildable application | ✅ full workspace `cargo check`/`build` clean on aarch64-pc-windows-msvc |
| Windows ARM64 build | ✅ native `lw.exe` (0xAA64); Tauri app compiles; NSIS release workflow |
| Working local Parakeet transcription | ✅ verified (CPU + NPU), WER ~4–5% |
| Global hotkey | ✅ `lw-platform` WH_KEYBOARD_LL hook (compiles) + Tauri global-shortcut (wired) |
| Recording overlay | ✅ transparent non-activating Tauri overlay window + state machine |
| Automatic text insertion | ✅ `lw-platform` clipboard-paste injector (compiles); wired into the app worker |
| Settings | ✅ typed, atomic, forward-compatible; Settings UI panel |
| Model management | ✅ SHA-256-pinned manifest, resumable downloader, atomic promote (tested) + CLI/script |
| Diagnostics | ✅ `lw diagnose` / app Diagnostics panel; honest provider/device/NPU report |
| Logs | ✅ `tracing`; no audio/transcript/clipboard/key leakage |
| Dictionary | ✅ case-aware phrase/word replacement (tested) + Settings UI |
| Optional text cleanup | ✅ deterministic + optional OpenAI-compatible LLM (raw-fallback, tested) |
| Backend abstraction | ✅ `SpeechEngine`/`EncoderBackend` traits; Parakeet + Whisper adapter |
| Tested CPU fallback | ✅ verified independently (QNN absent → CPU transcribes) |
| Investigated X2 NPU path | ✅ [`x2-npu.md`](x2-npu.md) |
| NPU implementation (if possible) | ✅ **implemented and verified** on HTP V81 |
| macOS support / path | ◑ builds; ANE path designed, not verified (no hardware) |
| Linux scaffolding | ✅ platform layer stubs compile |
| Benchmarks | ✅ [`benchmarks.md`](benchmarks.md), measured on X2 |
| Tests | ✅ ~140 unit + integration; all green |
| Documentation | ✅ research/architecture/x2-npu/benchmarks/licenses/build/FINAL_REPORT |
| Licensing documentation | ✅ [`licenses.md`](licenses.md) + `THIRD_PARTY_NOTICES.md` |
| Build/release instructions | ✅ [`build.md`](build.md) + scripts + CI/release workflows |

## 1. What was implemented

A cross-platform, local speech-to-text dictation application, Windows-ARM64-first, built as a Rust
workspace + Tauri 2 desktop shell:

- **`lw-core`** — hardware/UI-agnostic core: audio buffers + ring buffer + resampler; a
  model-agnostic VAD endpoint state machine; the `SpeechEngine` trait + backend-selection policy;
  a text pipeline (identity / deterministic cleanup / dictionary / optional OpenAI-compatible LLM);
  a case-aware replacement dictionary; per-application profiles + matcher; typed atomic settings;
  a SHA-256-pinned, resumable model manager; hardware-capability data; diagnostics.
- **`lw-ort`** — ONNX Runtime layer: dynamic loading of a stock `onnxruntime.dll`, lazy QNN
  plugin-EP registration, NPU device enumeration, CPU + QNN/HTP session builders with on-device
  EPContext caching, and runtime-manifest verification.
- **`lw-engine-parakeet`** — NVIDIA Parakeet TDT 0.6B v3 engine: a Rust mel front end (ONNX
  `nemo128` preprocessor + a native Rust reimplementation), a pluggable encoder backend
  (CPU int8/fp32 or Qualcomm HTP fp16), a TDT greedy decoder, and word-overlap chunk merging.
- **`lw-vad-silero`** — Silero VAD (ONNX) implementing the core VAD trait.
- **`lw-engine-whisper`** — a Whisper `SpeechEngine` adapter (abstraction complete; backend not
  wired — reports unavailable).
- **`lw-platform`** — OS integration traits (audio capture, global hotkey, text injection,
  clipboard, capabilities, secure storage) with Windows implementations and macOS/Linux modules.
- **`lw-engine-sherpa`** — a portable CPU engine (Whisper / Moonshine / SenseVoice / NeMo
  transducer) via sherpa-onnx, behind the optional `sherpa` feature. Off by default; see
  [`licenses.md`](licenses.md) for the espeak-ng GPL-3.0 trap that makes the default archive
  unshippable alongside the proprietary Qualcomm runtime.
- **`lw_core::model::catalog`** — a pre-download catalog: per model, size, languages, quality and
  speed tiers, supported hardware, and a hardware-aware recommender. It keeps **estimates** and
  **measurements** in separate fields, and every measurement carries the machine it was taken on.
- **`lw_core::bench`** — the measurement core (clip loading, `measure()`, word-weighted WER),
  shared by the CLI and the app so the two can never disagree about what a number means.
- **`lw-cli`** (`lw`) — `diagnose`, `transcribe`, `bench`, `bench --quick`, `devices`, `record`,
  `selfcheck`, `models list|info|install|compare`.
- **`app/`** — Tauri 2 shell (tray, settings window, non-activating overlay, global shortcut,
  typed command/event IPC) + React/TypeScript UI (Dictate / Settings / Models / Diagnostics /
  Benchmark), with a background dictation worker wiring hotkey → mic capture → engine → text
  pipeline → injection. Three hotkey modes (push-to-talk, toggle, hands-free), an in-app model
  picker with verified download, and an in-app benchmark. See [`using.md`](using.md).
- Docs (`research.md`, `architecture.md`, `x2-npu.md`, `benchmarks.md`, `licenses.md`, `build.md`),
  scripts (model download, runtime fetch, ARM64 build, QNN probe/quantize, benchmark), a pinned
  model manifest, and CC-BY-4.0 FLEURS test fixtures.

## 2. What works (verified on the X2)

- **Native ARM64 build.** `lw.exe` is `aarch64` (machine 0xAA64); no x86 emulation for our code.
- **Parakeet transcription on CPU** — `lw bench` over 12 FLEURS clips (en/ru/es/uk):
  **mean RTF 0.032, word-weighted WER 5.4 %**. Correct sentences in all four languages.
- **Parakeet encoder on the Snapdragon X2 Hexagon NPU (QNN HTP V81)** — the full encoder compiles
  to one EPContext node via on-device prepare (fp16), runs at ~23 ms/window, and the end-to-end
  `lw bench` NPU path is **mean RTF 0.0145, WER 4.8 %**. The 1.2 GB context binary is cached and
  reloads in ~2 s. This is real NPU execution, reported honestly (`provider=QNN`,
  `acceleration=NPU`, device `Snapdragon Hexagon HTP (V81)`).
- **Parakeet encoder on the GPU** — via ONNX Runtime's WebGPU plugin EP (Dawn → D3D12), measured
  on the X2's integrated Adreno at **mean RTF 0.0609, WER 5.4 %**, correct transcripts in all four
  languages. This path is vendor-neutral and needs no model artifact of its own, so the same code
  serves NVIDIA, AMD, Intel and Apple GPUs — though only the Adreno has been run.
- **Automatic backend selection with honest reporting** — `diagnose` enumerates the CPU + QNN NPU
  devices; `--backend auto` uses the NPU when present and falls back to CPU, never mislabeling CPU
  as NPU.
- **The CPU fallback is genuinely independent** — with the QNN DLL absent, the CPU path transcribes
  identically. (A real ORT gotcha was fixed here: a registered QNN EP auto-applies to CPU sessions
  and fails the encoder's dynamic `/Expand`; CPU sessions are now pinned to the CPU device.)
- **The desktop app on the NPU** — the app's own benchmark path, run against the model installed
  in its app-data directory, reports `QNN on NPU` at warm RTF **0.0099** (vs 0.0357 on the CPU).
  Getting there uncovered a real bug: `OrtRuntime` tracked EP registration per instance while
  ONNX Runtime's environment is global, so a second handle's registration was refused and the
  caller read that as "no NPU" and fell back to the CPU. The runtime is now one handle per
  process, with a regression test.
- **Unit tests**: 263 across the workspace, all passing, plus ignored integration tests that need
  a real model (`transcribe_fixture`, the app's benchmark path, and the ORT singleton check).

## 3. What was tested

| Area | How | Result |
|---|---|---|
| Audio downmix / RMS / resample / ring buffer | unit tests | pass |
| VAD endpoint state machine (onset/hangover/pre-roll/max-cut) | unit tests | pass |
| Backend selection policy (auto/force, NPU→CPU fallback) | unit tests | pass |
| Dictionary (phrase/word, case-aware, longest-first) | unit tests | pass |
| Text cleanup + LLM fallback-to-raw | unit tests (incl. unreachable-endpoint) | pass |
| Settings load/save/atomic/forward-compat | unit tests | pass |
| Model manifest validation + SHA-256 verify + atomic promote | unit tests | pass |
| Runtime manifest verify | unit tests | pass |
| Vocab parse + TDT detokenize | unit tests | pass |
| Chunk merge (overlap, trailing-drop) | unit tests | pass |
| Mel (native) shape/normalization + Slaney filterbank | unit tests | pass |
| ORT dynamic load + QNN registration + device enum | run on X2 (`diagnose`) | pass |
| **End-to-end CPU transcription** | `lw transcribe`/`bench` + integration test on X2 | pass, WER 5.4 % |
| **End-to-end NPU (HTP V81) transcription** | `lw bench` on X2 | pass, WER 4.8 %, RTF 0.0145 |
| **End-to-end GPU transcription** | `lw bench --backend webgpu` on X2 (Adreno) | pass, WER 5.4 %, RTF 0.0609 |
| Accelerator vocabulary (ids, serde, ordering, artifact rule) | unit tests | pass |
| Provider probe reports present / registered / devices separately | `lw diagnose` on X2 | pass |
| Runtime staging into the installer bundle | `build.rs` on X2 (20 files staged) | pass |
| Model catalog JSON shape + estimate/measurement separation | unit tests | pass |
| Benchmark arithmetic (cold/warm split, word-weighted WER) | unit tests with a scripted mock engine | pass |
| Install state detection against the app's real model directory | `LW_MODELS_ROOT=<app dir> lw models list` on X2 | pass (`INSTALLED: yes`) |
| Hotkey resolution, and that validation rejects exactly what cannot be registered | unit tests | pass |
| **The app's benchmark path end to end** | ignored integration test against the installed model on X2 | pass, `QNN on NPU`, warm RTF 0.0099 |
| **The app running with all five tabs** | launched on X2, window captured | pass |

| **Live microphone capture → transcribe** | `lw record` on X2 (Aqstic array, 48 kHz→16 kHz) | pass, empty on silence (correct), no crash |

Not tested at runtime: text injection into live apps, and clicking through the GUI (model
download, benchmark button, hotkey editor) — the session running this work has no composited
desktop, so screen capture returns black and only `PrintWindow` renders, which cannot be trusted
for layout. What **is** runtime-verified: the app launches with all five tabs, its settings and
hotkey IPC answer correctly on screen, and its benchmark path was driven end to end against the
real model through an integration test. The identical capture→engine→text path is runtime-verified
via `lw record`. **The frontend panels themselves have been type-checked and built, not clicked** —
that verification needs a human at the machine. macOS/Linux were not executed (no hardware).

## 4. Exact supported platforms

| Platform | Installer | State |
|---|---|---|
| Windows 11 ARM64 (Snapdragon X2) | `.exe` (NSIS), `.msi` | **first-class, verified** — CPU + Qualcomm NPU + GPU |
| Windows 11 ARM64 (Snapdragon X Elite / V73) | same | expected to work; the NPU needs a V73 context, untested here |
| Windows x64 (Intel / AMD) | `.exe`, `.msi` | builds; CPU + GPU implemented, not executed |
| macOS 11+ Apple Silicon | `.dmg`, `.app` | builds; CPU + GPU implemented, CoreML/ANE not implemented; not executed |
| Linux x64 | `.deb`, `.rpm`, `.AppImage` | builds; CPU + GPU implemented; platform layer partial (hotkey/injection) |
| Linux ARM64 | — | builds; CPU only (no ORT WebGPU build for that RID) |

Packaging is configured for every row and the release workflow is a four-platform matrix, with a
check that fails the build if the proprietary Qualcomm libraries reach a non-Snapdragon package.
**Only the Windows ARM64 artifact has been built and run on real hardware.** Code signing
(Windows) and signing + notarization (macOS) remain.

## 5. Exact backend matrix

Nine execution providers behind one vocabulary (`lw_core::capabilities::Accelerator`); settings,
detection, the engine, the CLI and the UI all read it. Measured rows are 12 FLEURS clips on the
X2 via `lw bench`.

| Accelerator | Provider library | Bundled | Own model artifact | Status |
|---|---|---|---|---|
| CPU | built in | — | no | **verified** — RTF 0.0324, WER 5.4 % |
| Qualcomm NPU | `onnxruntime_providers_qnn.dll` | ✅ win-arm64 | **yes** (HTP context per Hexagon gen) | **verified** — RTF 0.0160, WER 4.8 % |
| GPU (WebGPU) | `onnxruntime_providers_webgpu.dll` | ✅ all 4 platforms | no | **verified on Adreno** — RTF 0.0609, WER 5.4 % |
| NVIDIA CUDA | `onnxruntime_providers_cuda.dll` | ❌ | no | implemented; needs CUDA 12 + cuDNN 9 |
| NVIDIA TensorRT | `onnxruntime_providers_tensorrt.dll` | ❌ | no | implemented; needs TensorRT 10 |
| DirectML | `onnxruntime_providers_dml.dll` | ❌ | no | implemented; needs the DML redistributable |
| Apple CoreML / ANE | `libonnxruntime_providers_coreml.dylib` | ❌ | effectively yes | implemented; needs a static fp16 export |
| Intel OpenVINO | `onnxruntime_providers_openvino.dll` | ❌ | **yes** for the NPU | implemented; needs the OpenVINO runtime |
| AMD Vitis AI | `onnxruntime_providers_vitisai.dll` | ❌ | **yes** | implemented; needs the Ryzen AI SDK |

The unbundled providers are wired through selection, settings, diagnostics and the benchmark
already: dropping the library into the runtime directory makes one appear as usable, with no code
change. They are not shipped because each needs a vendor SDK and **none could be verified here**.
An NPU additionally needs a quantized artifact compiled and validated on that silicon — that, not
the code, is what gates a new NPU. See [`hardware.md`](hardware.md).

| Engine | Provider | Status |
|---|---|---|
| Parakeet TDT v3 | QNN / ORT CPU / WebGPU | **verified on X2** |
| Whisper, Moonshine, SenseVoice, NeMo transducer | sherpa-onnx (CPU) | behind the optional `sherpa` feature |

## 6. Exact X2 NPU status

**Achieved:** Parakeet TDT 0.6B v3's FastConformer encoder runs on the Snapdragon X2 Elite Hexagon
HTP (V81) from a native ARM64 process via ONNX Runtime's QNN plugin execution provider, using an
on-device-prepared, cached fp16 EPContext binary; decode/mel stay on CPU; the full pipeline produces
correct transcriptions at RTF ≈ 0.015. Full procedure and evidence: [`x2-npu.md`](x2-npu.md).

**Honesty caveats:**
- The NPU artifact used is a **static-shape fp32 encoder** frozen to a 20 s window (mask
  constant-folded) and prepared to fp16 on-device. INT8/INT16 QDQ (smaller/faster) is scripted but
  not the verified default.
- The Python-side exploration earlier in the project ran under x64 emulation (the dev-box Python is
  emulated); the **Rust** results above are native ARM64 and are the ones that count.
- Reusing a context binary across *different* X2 machines is expected but unproven (one machine).

## 7. Performance measurements

See [`benchmarks.md`](benchmarks.md). Headline (X2, 12 FLEURS clips): CPU RTF 0.032 / WER 5.4 %;
NPU RTF 0.0145 / WER 4.8 %; encoder-only on HTP 23 ms per 10 s window (RTF 0.0023); Silero VAD
0.167 ms per 32 ms frame.

## 8. Known issues

- QNN's native libraries print HTP graph-prepare logs to stdout during NPU prepare; the app should
  suppress/redirect these (currently visible on the CLI's first NPU run).
- The NPU encoder uses a fixed static window (20 s); very short utterances still pay the full window
  of encoder compute (cheap on NPU, ~53 ms).
- English WER on FLEURS is inflated by number-word/casing normalization (e.g. "3" vs "three"), not
  recognition error.
- Text injection, hotkey capture, overlay, and the audio→engine wiring in the Tauri app are
  implemented as modules/seams but not yet exercised end-to-end in a live session.
- macOS/Linux are unverified.

## 9. Remaining work

- Runtime-verify the Tauri GUI hotkey→inject loop on a live desktop session (the loop is wired and
  compiles; only headless verification was impossible here). Validate text injection into real apps
  (VS Code, terminals, browsers) and hotkey capture across focus changes.
- Produce and publish a V81 INT8/INT16 QDQ encoder (smaller than the 1.2 GB fp16 context) and/or a
  pre-prepared context users can download to skip on-device prepare.
- Verify macOS (ORT CoreML EP or a FluidAudio CoreML bridge) on real Apple Silicon; flesh out the
  Linux platform layer (Wayland portal hotkeys, injection).
- Code-signing/notarization; MSIX/Store packaging; the updater.
- Streaming/partial results (a cache-aware or Nemotron streaming model) as a later engine.
- **DEBT, accepted deliberately: a Python sidecar runner for models that ship no ONNX export.**
  Principle 5 in [`architecture.md`](architecture.md) says no Python in the shipped runtime, and it
  still holds for everything built so far. But the reason several interesting models are missing
  from the catalog is not that they are bad -- it is that their only runner is a Python package
  (HojoAI's `hojo-asr` on safetensors, the full Qwen3-ASR checkpoints, most research ASR). The
  owner has allowed a Python sidecar for that case specifically, with a caveat, so it is recorded
  here rather than done silently. What it would cost, so that nobody is surprised later:
  - It will be SLOWER, and that is the part to say out loud in the UI, not only here. A sidecar
    means a process boundary and a serialization hop per utterance on top of PyTorch's own startup;
    an engine that loads in-process today would become an engine that has to be spawned, fed and
    waited on.
  - No NPU, and most likely no GPU. The QNN path is reached through ONNX Runtime EPs; a PyTorch
    sidecar does not get there, so such a model is CPU-bound on this hardware by construction.
  - It is a heavy runtime dependency to install, sign and ship, on a platform (Windows ARM64) where
    wheels are the least available.
  Therefore, if it is ever built: opt-in per model, never the default engine, never the fallback a
  user lands on without choosing it, and labelled in the model list as slower with the reason --
  the same rule the rest of this project applies to accelerators, applied to runners.

## 10. Build commands

See [`build.md`](build.md). Short form (Windows ARM64):
```
pwsh -File scripts\runtime\fetch-runtime.ps1
source ~/.msvc-arm64/env-arm64.sh   # portable-toolset dev machines
cargo build --release -p lw-cli --target aarch64-pc-windows-msvc
pwsh -File scripts\build\build-windows-arm64.ps1 -Release   # + Tauri app
```

## 11. Run commands
```
lw --runtime-dir runtime/win-arm64 diagnose
lw --runtime-dir runtime/win-arm64 transcribe clip.wav --model-dir <models>/parakeet-tdt-0.6b-v3 --backend auto
lw --runtime-dir runtime/win-arm64 bench tests/fixtures/audio --model-dir <models>/parakeet-tdt-0.6b-v3 --backend npu
```

## 12. Model installation
```
python scripts/models/download_model.py models/manifests/parakeet-tdt-0.6b-v3.json \
    --dest "$LOCALAPPDATA/LocalWisper/models" --target cpu_int8
```
CPU needs `nemo128.onnx`, `vocab.txt`, `decoder_joint-model.int8.onnx`, `encoder-model.int8.onnx`.
NPU additionally needs a static-shape encoder (`encoder-static-tNNNN.onnx` + `.data`); the app
prepares/caches its HTP context on first use. Models are CC-BY-4.0 (NVIDIA / istupakov) and never
committed to git.

## 13. Architecture overview

See [`architecture.md`](architecture.md). In one line: a Rust core owns the whole real-time pipeline
(capture → VAD → mel → encoder backend → TDT decode → text pipeline → injection) behind the
`SpeechEngine`/`EncoderBackend`/platform traits; Tauri renders state and settings over a small typed
IPC; audio never crosses the IPC boundary; the inference backend is selected at runtime and reported
truthfully.

## 14. Licensing

See [`licenses.md`](licenses.md). App code Apache-2.0; Parakeet CC-BY-4.0 (attributed); ORT + QNN EP
MIT; Qualcomm QNN runtime under the Qualcomm AI Stack License (object-code-only, PDF shipped); Silero
MIT; all Rust/JS deps permissive. No GPL/AGPL code is linked.

## 15. Security considerations

HTTPS-only, SHA-256-pinned model downloads with atomic promotion; no downloaded binary is executed;
no telemetry; no network unless model download or the optional LLM endpoint is enabled; API keys via
the OS keychain (`keyring`); logs never contain audio/transcripts/clipboard/keys. The QNN runtime is
bundled in the installer (not fetched standalone), per its licence, and excluded from non-Snapdragon
packages.
