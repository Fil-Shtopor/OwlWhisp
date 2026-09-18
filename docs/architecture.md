# LocalWisper — Architecture

_Companion to [`research.md`](research.md). This document defines the module boundaries, the core
traits, the data flow, and the concurrency model. It is meant to be stable: individual engines,
platforms and models can change without touching the rest._

## 1. Principles

1. **UI is not the pipeline.** The Rust core owns audio capture, VAD, inference, the text pipeline
   and OS integration. The Tauri/React frontend renders state and edits settings. **Audio samples
   never cross the IPC boundary.**
2. **Backends are pluggable and honest.** A `SpeechEngine` is selected at runtime by a capability
   detector; diagnostics always report the *actual* backend/provider/device. CPU fallback is never
   removed.
3. **One inference runtime.** ONNX Runtime (via the `ort` crate, `load-dynamic`) is the single ML
   runtime, and every accelerator reaches it the same way — as a **plugin execution provider**
   registered by name (`lw_core::capabilities::Accelerator` is the vocabulary: CPU, Qualcomm NPU,
   WebGPU, CUDA, TensorRT, DirectML, CoreML, OpenVINO, Vitis AI). Adding a vendor is a variant plus
   a staged library, not a new runtime. This keeps the mel front end and TDT decoder identical
   across backends.

   The one asymmetry worth knowing: a GPU consumes the ordinary graph, so one artifact serves every
   GPU vendor, while an NPU wants a graph quantized and compiled for that silicon
   (`Accelerator::needs_dedicated_artifact`). That is why GPU coverage generalizes and NPU coverage
   has to be earned per vendor.
4. **Native ARM64 first.** The app's own code is native `aarch64-pc-windows-msvc`; no x86 emulation
   for our binary. Dependencies are native ARM64 wherever a build exists.
5. **No Python in the shipped runtime.** Python is used only for model conversion / QNN compilation /
   benchmarks under `scripts/`. This holds for every engine that exists today. One exception has
   been *allowed but not built*: a Python sidecar for models published with no ONNX export at all,
   which would be opt-in per model, CPU-only, and labelled as slower -- see
   [`FINAL_REPORT.md` section 9](FINAL_REPORT.md) for what it costs.

## 2. Crate map (Rust workspace)

```
crates/
  lw-core/              # pure logic, no OS/UI deps
    audio/              #   ring buffer, resampler wrapper, format conversion
    vad/                #   VAD trait + end-of-utterance state machine (model-agnostic)
    engine/             #   SpeechEngine trait, EngineRegistry, backend selection policy
    text/               #   TextProcessor trait + pipeline (normalize, dictionary, punctuation, LLM)
    dictionary/         #   replacement table (exact + phrase, case-aware)
    profiles/           #   per-application profile model + matcher
    settings/           #   typed settings, atomic load/save, schema
    model/              #   model manifest, registry, downloader, SHA-256 verify, cache
    diagnostics/        #   capability report, latency metrics, redacted diag bundle
    capabilities/       #   OS/CPU/NPU detection data model
  lw-ort/               # ONNX Runtime layer: dynamic load, plugin-EP registration (any vendor),
                        #   runtime-manifest verification, session helpers, EPContext cache
  lw-vad-silero/        # Silero VAD implementation of lw-core::vad::Vad on ort
  lw-engine-parakeet/   # Parakeet TDT engine: Rust mel front end + encoder backend + TDT greedy
  lw-engine-whisper/    # Whisper adapter (optional fallback; thin, feature-gated)
  lw-platform/          # OS integration traits + windows/ macos/ linux/ impls
  lw-cli/               # `lw` command-line: diagnose, bench, transcribe, models, self-check
app/
  src-tauri/            # Tauri 2 shell: commands, events, tray, windows, wires core to UI
  frontend/             # React + TypeScript + Vite UI
```

Dependency direction is strictly downward: `lw-core` depends on nothing OS-specific; `lw-ort`,
`lw-vad-silero`, `lw-engine-*` depend on `lw-core`; `lw-platform` depends on `lw-core`; `lw-cli` and
`app/src-tauri` compose everything.

## 3. The `SpeechEngine` trait

```rust
pub trait SpeechEngine: Send + Sync {
    fn backend_name(&self) -> &str;              // "parakeet-tdt-0.6b-v3"
    fn provider(&self) -> Provider;              // QnnHtp | OnnxCpu | CoreML | ...
    fn device(&self) -> DeviceInfo;              // "Snapdragon X2 Elite HTP (V81)" | "CPU (18 threads)"
    fn acceleration(&self) -> Acceleration;      // Npu | Cpu | Gpu | Ane
    fn supported_languages(&self) -> &[Language];
    fn supports_streaming(&self) -> bool;

    fn initialize(&mut self, ctx: &EngineInitContext) -> Result<()>;
    fn health_check(&self) -> HealthReport;      // runs a tiny inference, reports latency & provider
    fn transcribe(&mut self, audio: &AudioBuffer) -> Result<Transcript>;   // whole utterance
    fn start_stream(&mut self) -> Result<Box<dyn TranscribeStream>>;       // Err if !supports_streaming
    fn shutdown(&mut self);
}
```

`Provider`, `Acceleration` and `DeviceInfo` are the honesty contract: `health_check()` actually
executes on the claimed device and the report is surfaced verbatim in the Diagnostics screen. An
engine that *wants* the NPU but had to fall back reports `Provider::OnnxCpu`, never `QnnHtp`.

### 3.1 Parakeet engine internals

`ParakeetEngine` is composed of three stages, each independently swappable:

- **Feature front end** (`lw-engine-parakeet::mel`): Rust implementation of NeMo's
  `FilterbankFeatures` — 16 kHz mono, pre-emphasis 0.97, centre-padded Hann(400) in a 512-pt FFT,
  hop 160, 128 Slaney mel bins (fmin 0, fmax 8000, area-normalized), `ln(x + 2⁻²⁴)`, per-feature
  mean/unbiased-std normalization. Runs on CPU always (the ONNX STFT op is CPU-only on every EP).
- **Encoder backend** (`EncoderBackend` trait): the only stage that moves between devices.
  - `StaticWindowEncoder` — a static-shape graph over a fixed mel window. Built either by
    `build_qnn_session` (Hexagon NPU, fp16, with an on-device-prepared EPContext binary cached per
    model hash / QAIRT version / HTP arch) or by `build_accel_session` on any other provider.
  - `CpuEncoder` — a dynamic-shape graph, on the CPU EP (the universal fallback) or on any other
    provider via `build_accel_session`. Static shapes were only ever the HTP's requirement, so a
    GPU takes this path and processes a clip in one pass.

  Which one runs is decided by `ParakeetEngine::acceleration_plan()` from the user's
  `BackendPreference` intersected with the accelerators ONNX Runtime actually enumerates. The
  chosen accelerator is reported back through `SpeechEngine::accelerator()` as a stable id, and
  the reasons — including any fallback — through `SpeechEngine::notes()`.
- **TDT greedy decoder** (`lw-engine-parakeet::tdt`): Rust port of NeMo `GreedyTDTInfer`. Runs the
  fused `decoder_joint` ONNX on CPU; splits joint logits into 8193 token + 5 duration outputs,
  argmaxes both, advances the LSTM state only on non-blank, skips `duration` encoder frames, caps at
  10 symbols/frame. Emits tokens with 80 ms timestamps; detokenizes `vocab.txt` (`▁` → space,
  `<blk>` = blank id 8192).

Backend selection order (configurable; "Automatic" is the default):
`QnnHtp` → `CoreMl` (macOS) → `OrtCpu`. A backend that fails `health_check()` is skipped and the
reason recorded.

## 4. Audio & VAD data flow

```
mic ──cpal(WASAPI/CoreAudio/ALSA)──▶ capture thread
   │  native format f32/i16 → mono downmix → push to
   ▼
16 kHz mono ring buffer (Arc, lock-light)  ◀── resampler (rubato) if device ≠ 16 kHz
   │
   ├─▶ VAD stage (Silero, 512-sample hops) ── annotates a speech timeline (SpeechSegment{start,end})
   │
   └─▶ level meter (RMS) ── throttled to overlay via a Channel (~30 Hz)

on stop/end-of-utterance:
   segment(s) ──▶ SpeechEngine.transcribe() ──▶ Transcript
                                                   │
                                                   ▼
                                        TextPipeline (normalize→dictionary→punct→[LLM])
                                                   │
                                                   ▼
                                        TextInjector (clipboard-paste / type)
```

The **VAD is independent of the STT model** (principle: a `Vad` trait over the shared ring buffer).
It drives three things: end-of-utterance detection in hands-free/toggle mode, leading/trailing
silence trimming, and cutting long recordings at silences for chunked encoding. Push-to-talk uses
VAD only for trimming and empty-recording detection — the key press/release define the boundaries.

**Chunking rule (long audio):** never feed more than the encoder's static window (default 20 s on
V81, dynamic on CPU) in one call; split at VAD silences with ~1 s overlap and merge transcripts by
word-level overlap (drop up to 6 trailing words as right-edge hallucination), the OpenWritr merge.

## 5. Recording state machine

```
Idle ──hotkey.press──▶ Arming (open mic, ensure model resident) ──first audio──▶ Listening
Listening ──(PTT release | toggle again | hands-free VAD end | max-duration)──▶ Transcribing
Transcribing ──▶ [Polishing?] ──▶ Delivering ──▶ Delivered → Idle
   any ──Esc/cancel──▶ Idle (confirm if recording > 30 s)
   any error ──▶ Error(tier) → Idle    (tier ∈ {audio, model, paste})
```

Overlay states map 1:1 to `IDLE / LISTENING / PROCESSING / DONE / ERROR`. Cancel is inert during
Transcribing except to hide the overlay. Delivery is bound to the foreground window captured at
`Arming`; if it changed, the text goes to the clipboard with a warning instead of being injected.

## 6. Platform abstraction

Traits live in `lw-platform`; each has `windows`, `macos`, `linux` modules behind `#[cfg]`.

| Trait | Windows | macOS | Linux |
|---|---|---|---|
| `AudioCapture` | cpal/WASAPI | cpal/CoreAudio | cpal/ALSA-PipeWire |
| `GlobalHotkey` | `WH_KEYBOARD_LL` hook (press/release, modifier-only) + `global-hotkey` fallback | Carbon + CGEventTap | X11 XGrabKey / portal (stub) |
| `TextInjector` | clipboard + Ctrl+V (SendInput VK 0x56) with OLE snapshot/restore + marker | pasteboard + Cmd+V (AX-aware) | XTest / portal (stub) |
| `Clipboard` | Win32 OLE + exclusion formats | NSPasteboard | wl-clipboard/x11 (stub) |
| `OverlayWindow` | Tauri window `focusable(false)`→WS_EX_NOACTIVATE + SWP_NOACTIVATE | tauri-nspanel | plain window (stub) |
| `SystemTray` | Tauri `TrayIconBuilder` | same | same |
| `PlatformCapabilities` | registry CPU string + NPU device enum + HTP-arch from DriverStore | sysctl + CoreML availability | /proc + ORT providers |

`TextInjector` prefers, in order: (1) save clipboard, (2) set our text with history/cloud-exclusion
markers, (3) paste, (4) restore only if our payload is still current (clipboard sequence number +
private marker). It never destroys the user's clipboard.

Linux modules compile and return `Unsupported`/no-op with a clear message so the workspace always
builds; they are marked as scaffolding in the FINAL_REPORT.

## 7. IPC (Tauri) contract

- **Commands** (frontend → core): `get_settings`, `set_settings`, `get_diagnostics`,
  `list_models`, `install_model`, `set_backend`, `start_benchmark`, `list_devices`,
  `set_dictionary`, `list_profiles`, … — small, typed, low-frequency.
- **Events** (core → frontend): `state_changed` (recording state), `level` (mic RMS, throttled),
  `partial` / `final` transcript, `model_progress`, `error`. High-frequency streams use
  `tauri::ipc::Channel<T>`.
- Types are shared via `tauri-specta` (generated TypeScript). No `any` on the TS side.

## 8. Concurrency model

- **Audio callback thread** (cpal): only downmix + push to ring buffer + RMS; no allocation-heavy
  work, no locks held across the callback.
- **Hotkey hook thread** (Windows `WH_KEYBOARD_LL`): lock-free bitmap, returns well under the
  `LowLevelHooksTimeout`; posts events to the core.
- **Inference worker** (dedicated thread, large stack): owns the `SpeechEngine`; receives segments,
  runs mel + encoder + decode, emits transcripts. ONNX sessions are created here and never shared
  across the EP-registration boundary.
- **Tokio runtime**: model downloads, LLM HTTP calls, file I/O.
- **Main/UI thread** (Tauri): window and tray operations (some OS calls must run here; use
  `run_on_main_thread`).

## 9. Security & privacy (see [`licenses.md`](licenses.md), FINAL_REPORT §Security)

- Model downloads are HTTPS-only, SHA-256-pinned against a committed manifest, size-checked,
  resumable, atomically promoted. No downloaded binary is executed.
- No telemetry. No network at all unless the user enables model download or an optional LLM endpoint.
- API keys stored via `keyring` (Windows Credential Manager / macOS Keychain / Secret Service).
- Logs never contain audio, transcript text, clipboard contents, or keys unless a "secure debug"
  toggle is explicitly enabled.

## 10. Where each research finding landed

- QNN plugin-EP loading, EPContext cache, honest device reporting → `lw-ort` + `QnnHtpEncoder`.
- NeMo-exact mel + real TDT-duration decode (fixing the two ecosystem bugs) → `lw-engine-parakeet`.
- WH_KEYBOARD_LL PTT + clipboard-restore protocol + foreground-target guard → `lw-platform::windows`.
- Capability-based hardware gating (never CPU-string) → `lw-core::capabilities` + `lw-platform`.
- Silero VAD params, pre-roll/hangover state machine → `lw-vad-silero` + `lw-core::vad`.
- Superwhisper/Wispr/Handy UX (modes, two-layer vocabulary, status colours, keep-alive, overlay
  styles) → `lw-core::profiles`/`dictionary`/`settings` + frontend.
