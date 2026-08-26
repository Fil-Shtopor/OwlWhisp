# LocalWisper — Final Report

_Written 2026-08-26. This report is deliberately blunt about what is verified vs. inferred vs.
remaining. "Verified" means it was executed and observed on the target machine (Snapdragon X2 Elite
Extreme X2E94100, Windows 11 ARM64); "compiles" means it builds but was not runtime-exercised;
"stub/scaffold" means an implemented interface without a full backend._

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
- **`lw-cli`** (`lw`) — `diagnose`, `transcribe`, `bench`, `selfcheck`.
- **`app/`** — Tauri 2 shell (tray, settings window, non-activating overlay, global shortcut,
  typed command/event IPC) + React/TypeScript UI (Dictate / Settings / Diagnostics).
- Docs (`research.md`, `architecture.md`, `x2-npu.md`, `benchmarks.md`, `licenses.md`, `build.md`),
  scripts (model download, runtime fetch, ARM64 build, QNN probe/quantize, benchmark), a pinned
  model manifest, and CC-BY-4.0 FLEURS test fixtures.

## 2. What works (verified on the X2)

- **Native ARM64 build.** `lw.exe` is `aarch64` (machine 0xAA64); no x86 emulation for our code.
- **Parakeet transcription on CPU** — `lw bench` over 12 FLEURS clips (en/ru/es/uk):
  **mean RTF 0.032, word-weighted WER 4.2 %**. Correct sentences in all four languages.
- **Parakeet encoder on the Snapdragon X2 Hexagon NPU (QNN HTP V81)** — the full encoder compiles
  to one EPContext node via on-device prepare (fp16), runs at ~23 ms/window, and the end-to-end
  `lw bench` NPU path is **mean RTF 0.0145, WER 4.8 %**. The 1.2 GB context binary is cached and
  reloads in ~2 s. This is real NPU execution, reported honestly (`provider=QNN`,
  `acceleration=NPU`, device `Snapdragon Hexagon HTP (V81)`).
- **Automatic backend selection with honest reporting** — `diagnose` enumerates the CPU + QNN NPU
  devices; `--backend auto` uses the NPU when present and falls back to CPU, never mislabeling CPU
  as NPU.
- **The CPU fallback is genuinely independent** — with the QNN DLL absent, the CPU path transcribes
  identically. (A real ORT gotcha was fixed here: a registered QNN EP auto-applies to CPU sessions
  and fails the encoder's dynamic `/Expand`; CPU sessions are now pinned to the CPU device.)
- **Unit tests**: 70 (lw-core) + 9 (lw-ort) + 17 (lw-engine-parakeet) + 1 (whisper) + VAD/CLI, all
  passing. **Integration test**: the ignored `transcribe_fixture` passes with the real model
  (WER < 0.35 on the English fixture).

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
| **End-to-end CPU transcription** | `lw transcribe`/`bench` + integration test on X2 | pass, WER 4.2 % |
| **End-to-end NPU (HTP V81) transcription** | `lw bench` on X2 | pass, WER 4.8 %, RTF 0.0145 |

Not tested at runtime: text injection into live apps, global-hotkey capture in a live session, the
overlay window behavior, and the full Tauri app loop (audio-capture→engine wiring is a documented
seam). macOS/Linux were not executed (no hardware).

## 4. Exact supported platforms

| Platform | State |
|---|---|
| Windows 11 ARM64 (Snapdragon X2) | **first-class, verified** (CPU + NPU) |
| Windows 11 ARM64 (Snapdragon X Elite / V73) | expected to work on CPU; NPU needs a V73 context (untested here) |
| Windows x64 | builds; CPU only |
| macOS Apple Silicon | builds; CPU via ORT; ANE path designed but unverified |
| Linux x64/ARM64 | builds; platform layer scaffolded; ORT CPU |

## 5. Exact backend matrix

| Engine | Provider | Acceleration | Status |
|---|---|---|---|
| Parakeet TDT v3 | QNN | NPU (HTP V81) | **verified on X2** |
| Parakeet TDT v3 | ONNX Runtime | CPU (int8) | **verified on X2** |
| Parakeet TDT v3 | CoreML | ANE/GPU (macOS) | designed; unverified (no Mac) |
| Whisper | ONNX Runtime | CPU | adapter present; backend not wired |

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

See [`benchmarks.md`](benchmarks.md). Headline (X2, 12 FLEURS clips): CPU RTF 0.032 / WER 4.2 %;
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

- Wire `lw-platform` audio capture + hotkey + `lw-engine-parakeet` into the Tauri app's recording
  loop (the documented seam), so press-to-talk → transcribe → inject works live.
- Produce and publish a V81 INT8/INT16 QDQ encoder (smaller than the 1.2 GB fp16 context) and/or a
  pre-prepared context users can download to skip on-device prepare.
- Verify macOS (ORT CoreML EP or a FluidAudio CoreML bridge) on real Apple Silicon; flesh out the
  Linux platform layer (Wayland portal hotkeys, injection).
- Code-signing/notarization; MSIX/Store packaging; the updater.
- Streaming/partial results (a cache-aware or Nemotron streaming model) as a later engine.

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
