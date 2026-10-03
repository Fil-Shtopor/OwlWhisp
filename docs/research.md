# OwlWhisp — Phase 0 Research

_Compiled 2026-08-26. This document is the evidence base for every architectural decision in
[`architecture.md`](architecture.md). It draws on twelve deep-dive dossiers (kept in the project's
research archive) and on experiments run **on the actual target machine** — an ASUS Zenbook A16
with a **Snapdragon X2 Elite Extreme X2E94100** (Qualcomm Oryon, 18 cores, 48 GB), Windows 11
build 28000 ARM64._

Throughout, **[V]** marks a fact verified by reading source / official docs / release assets or by
running it locally; **[?]** marks something inferred or not yet verified on hardware. The
distinction is load-bearing: this project's first principle is *never claim hardware acceleration
that has not been observed*.

---

## 0. The headline result

**Parakeet TDT 0.6B v3 runs on the Snapdragon X2 Elite Hexagon NPU today, from a native ARM64
process, with no Qualcomm SDK login and no pre-compiled vendor artifact.** We proved it locally:

| Path | What ran | Result |
|---|---|---|
| ORT QNN EP plugin (`onnxruntime-qnn` 2.5.0, QAIRT 2.49.40) registered into native ARM64 `onnxruntime` 1.29.0 | tiny fp16 MatMul graph | executed on HTP, output matched CPU **[V]** |
| Same, full Parakeet encoder (istupakov fp32 export, static `[1,128,1000]`, `enable_htp_fp16_precision=1`) | 600 M-param FastConformer encoder | compiled to **one** EPContext node, online prepare **195 s**, inference **23 ms** / 10 s window (RTF **0.0023**), cosine **0.9993** vs CPU fp32 **[V]** |
| Same at `[1,128,2000]` (20 s window) | encoder | prepare **106 s**, inference **53 ms**, RTF **0.0026** **[V]** |
| End-to-end (mel → HTP encoder → CPU TDT decode) on 12 FLEURS clips (en/ru/es/uk) | full pipeline | **65–120 ms** per 5–8 s clip, word-weighted **WER ≈ 4.9 %** **[V]** |
| CPU baseline (ORT 1.29, sherpa int8 encoder, 8 threads) | encoder | RTF **≈ 0.085** **[V]** |

The X2 NPU is **not** device-gated away from us the way OpenWritr's published X-Elite artifact is:
`onnxruntime-qnn` 2.5.0 bundles the **V81** stub+skel (`QnnHtpV81Stub.dll`,
`libQnnHtpV81Skel.so`, `libqnnhtpv81.cat`) as ARM64X binaries that load into a native ARM64
process, and ORT's on-device graph prepare (`QnnHtpPrepare.dll`) builds the context binary for
*this* SoC. Full procedure and caveats: [`x2-npu.md`](x2-npu.md).

The consequence for the architecture: **the NPU is the default encoder backend on X2 Elite, not a
someday-maybe.** CPU int8 remains the universal fallback and is itself fast enough for dictation
(RTF ≈ 0.08–0.13 on the Oryon cores).

---

## 1. Project comparison

| Project | Lang / shell | Model(s) | NPU? | Win ARM64? | Licence | What we take |
|---|---|---|---|---|---|---|
| **OpenWritr** (trsdn) | Rust, winit+egui (no Tauri) | Parakeet v3 (ONNX), Whisper-L-v3-turbo | **Yes, HTP V73 (X Elite)** via ORT QNN EP | Yes (Store + per-user) | MIT | The whole QNN/Parakeet reference: graph surgery, INT8/INT16 recipe, EPContext wrapper, 3-manifest runtime pinning, clipboard-restore protocol, WH_KEYBOARD_LL hook, diagnostics. **[V]** |
| **Handy** (cjpais) | Rust, **Tauri 2** | Parakeet v3, Whisper, Moonshine (ONNX + GGUF) | No (CPU only, incl. on ARM64) | **Yes** (ships `arm64-setup.exe`) | MIT | Tauri overlay/tray/shortcut recipes, `handy-keys` PTT hook, PasteMethod enum, model-catalog + keep-alive UX, delayed-render clipboard. **[V]** |
| **OpenWhispr** | Electron+React, sherpa-onnx sidecars | Parakeet v3, Whisper, Nemotron | No | No (x64 only) | MIT | Sidecar supervisor discipline, fail-closed cloud routing, hard-15 s-cut *anti-pattern* to avoid. **[V]** |
| **Superwhisper** | native (closed) | Whisper, Parakeet v2/v3 (WhisperKit) | undocumented | Yes (x64+ARM64) | proprietary | UX only: modes, two-layer vocabulary, status-colour language, keep-alive. **[V docs]** |
| **Wispr Flow** | native (closed), cloud-only | cloud | n/a | **No (WoA unsupported)** | proprietary | UX only: PTT/double-tap-lock, Backtrack, auto-learn dictionary, shortcut validation rules. **[V docs]** |
| **VoiceInk** | Swift (macOS) | Whisper, Parakeet (FluidAudio/CoreML) | ANE (macOS) | n/a | **GPL-3.0** | UX ideas only — **no code reuse** (copyleft). **[V]** |
| **Whispering** (epicenter) | Svelte+Tauri | GGUF via transcribe-cpp | No | ? | **AGPL-3.0** | UX ideas only — **no code reuse**. **[V]** |
| **FluidAudio** | Swift (macOS) | Parakeet v3 CoreML | **ANE, verified** | n/a | Apache-2.0 | macOS acceleration reference + the CoreML `.mlmodelc` bundles. **[V]** |

**The gap we fill:** nobody ships an open-source, native **Windows-on-ARM** dictation app that uses
the Snapdragon NPU. OpenWritr is the closest (native ARM64 + NPU) but is X-Elite/V73-gated, not
Tauri, and targets the previous SoC generation. Handy is Tauri + ARM64 but CPU-only. We combine
Handy's Tauri chassis with OpenWritr's QNN engine, re-targeted to V81, behind a clean backend
abstraction.

---

## 2. Technology / backend comparison

### 2.1 Speech-engine runtime — three candidate strategies

| Strategy | CPU Parakeet | NPU Parakeet | macOS ANE | Effort | Verdict |
|---|---|---|---|---|---|
| **A. sherpa-onnx** via official Rust crate | ✅ mature, tested on `windows-11-arm` | ❌ its QNN path is POSIX-only, C++-only, Android-only, no X-series SoC | ❌ CoreML disabled in prebuilts | low | Good CPU fallback, **cannot reach our NPU** |
| **B. pykeio `ort` directly + own mel + own TDT decode** | ✅ (ORT 1.29 CPU EP) | ✅ **QNN plugin EP — verified locally** | ✅ CoreML EP option exists | medium-high | **Chosen.** Single runtime, we control shapes/quant, reaches HTP V81 |
| **C. `parakeet-rs` / `transcribe-rs` crates as deps | ✅ | ❌ no QNN | partial | low | `transcribe-rs` ignores TDT duration head (bug); take `parakeet-rs`'s **code** (mel+TDT) not the dep |

**Decision: Strategy B.** Depend on `ort` 2.0.0-rc.13 (ONNX Runtime 1.28/1.29), implement the mel
front end and TDT greedy decoder in Rust (ported and corrected from `parakeet-rs`, MIT), and reach
the NPU through ORT's plugin QNN EP. This is the only path that (a) reaches the X2 NPU, (b) keeps a
single runtime across CPU/NPU/CoreML, and (c) lets us fix the two known correctness bugs in the
crate ecosystem (see §2.3). sherpa-onnx remains available behind the same trait as an alternative
CPU engine.

### 2.2 ONNX Runtime linking on Windows ARM64 — the crux

- pyke's `ort-sys` prebuilt for `aarch64-pc-windows-msvc` ships a **static ORT 1.28 with DirectML**,
  no QNN. **[V]** So the QNN EP cannot come from the static build.
- The Qualcomm QNN EP is a **plugin** (`onnxruntime_providers_qnn.dll`) that registers into a stock
  `onnxruntime.dll` at runtime via `RegisterExecutionProviderLibrary` / `GetEpDevices` /
  `SessionOptionsAppendExecutionProvider_V2`. The `ort` crate exposes all three
  (`Environment::register_ep_library`, `EpDevice`, `SessionBuilder::with_auto_device`). **[V]**
- Therefore we build `lw-ort` with **`load-dynamic`** and ship a stock `onnxruntime.dll` (1.28.x /
  1.29.0 verified) plus the `onnxruntime-qnn` 2.5.0 EP DLL set. This is exactly how Qualcomm's own
  `ai-hub-apps` Windows samples load the EP. **[V]**

### 2.3 Two correctness bugs we must not inherit

1. `transcribe-rs` slices only the token logits and **ignores the 5 TDT duration outputs**, decoding
   Parakeet as if it were plain RNN-T (open PR #90). We implement the real TDT loop. **[V]**
2. `parakeet-rs` applies the 400-sample Hann window at the **start** of each 512-sample frame instead
   of **centre-padding** (56/56) as NeMo/onnx-asr do; not bit-exact. We centre-pad. **[V]**

### 2.4 Quantization for the NPU

- **fp16 (`enable_htp_fp16_precision=1`) on the fp32 graph** already works and is accurate
  (cosine 0.9993). This is our v1 NPU path — no calibration, no QDQ, produced by on-device prepare.
  **[V]**
- **INT8-weight / INT16-activation QDQ** (OpenWritr's AI-Hub recipe) is the memory/throughput
  optimization for later; it requires calibration and hit HTP op-config failures locally when done
  naïvely (`per_channel` → error 6020; INT8 activations → LayerNorm 3110). We keep the QDQ script
  (`scripts/qnn/quantize_encoder_qdq.py`) as an experiment, not the v1 default. **[V]**

---

## 3. Licensing findings (full text in [`licenses.md`](licenses.md))

| Component | SPDX / terms | Redistribution | 
|---|---|---|
| OwlWhisp app code | **Apache-2.0** | ours |
| NVIDIA Parakeet TDT 0.6B v3 weights (and istupakov / sherpa ONNX exports) | **CC-BY-4.0** | ✅ with attribution (name, link, licence URI, mark modifications) **[V]** |
| ONNX Runtime | **MIT** | ✅ **[V]** |
| `onnxruntime-qnn` EP (`onnxruntime_providers_qnn.dll`) | **MIT** | ✅ **[V]** |
| Qualcomm QNN/QAIRT runtime DLLs (`QnnHtp*.dll`, skel/stub) | **Qualcomm "AI Stack License"** (proprietary, no SPDX → `LicenseRef-Qualcomm-AI-Stack-License`) | ✅ **only** "in object code … as incorporated in Your software application"; never standalone; no reverse engineering; ship the PDF. **[V]** |
| Silero VAD | **MIT** | ✅ **[V]** |
| Tauri, cpal, ort, enigo, arboard, global-hotkey, handy-keys, rubato, keyring… | MIT / Apache-2.0 | ✅ **[V]** |
| TEN VAD | Apache-2.0 **+ non-compete rider** | excluded from default build **[V]** |
| VoiceInk (GPL-3.0), Whispering (AGPL-3.0) | copyleft | **no code reuse** — UX inspiration only **[V]** |

Consequence: bundle the Qualcomm DLLs **inside the installer** (not a separate download), ship
`Qualcomm_LICENSE.pdf` + ORT `ThirdPartyNotices.txt` + NVIDIA CC-BY attribution, and add a
"forbidden files" check so the Qualcomm binaries never leak into the x64 or non-Snapdragon packages
(OpenWritr's pattern).

---

## 4. Hardware compatibility matrix

| SoC | Hexagon HTP arch | QNN `soc_model` | NPU path | Verified |
|---|---|---|---|---|
| **Snapdragon X2 Elite (SC8480XP / X2E94100)** | **V81** | **88** | ORT QNN EP, `htp_arch=81` (online-prepare works with it unset too) | **[V] locally** |
| Snapdragon X / X Plus (SC8380XP, X1E/X1P) | V73 | 60 | ORT QNN EP `htp_arch=73` (OpenWritr's artifact) | [V] via OpenWritr |
| Apple Silicon (M-series) | ANE | — | ORT CoreML EP (static shapes) or FluidAudio `.mlmodelc` bridge | [?] not on our hardware |
| Intel/AMD x64, Linux x64/ARM64 | — | — | ORT CPU EP (int8) | [V] pattern, [?] on real Linux |

The mapping SoC→HTP arch is confirmed three ways for X2: the local driver package
(`libQnnHtpV81SkelDrv.so`), `ai-hub-models` `devices_and_chipsets.yaml`
(`qualcomm-snapdragon-x2-elite: htp_version 81, soc_model 88`), and `onnxruntime-qnn`
`soc_utils.cc` (`{"SC8480XP", 88}`). **[V]**

**Hardware detection rule:** never gate on the CPU marketing string (OpenWritr's substring gate
rejects "Snapdragon(R) X2 Elite Extreme"). Detect capability instead: enumerate `OrtEpDevice`s for
an NPU + `QNNExecutionProvider`, and read the HTP arch by globbing
`libQnnHtpV(\d\d)Skel*.so` in the `qcnspmcdm*` DriverStore package. **[V]**

---

## 5. Performance findings (measured on X2E94100 unless noted)

- Parakeet **encoder on HTP V81, fp16**: 23 ms / 10 s window, 53 ms / 20 s window → **RTF 0.002**.
  First-session **graph prepare is the cost**: ~195 s (10 s window) / ~106 s (20 s), producing a
  ~1.2 GB context binary that reloads in **~2.2 s**. ⇒ prepare once, cache the context binary. **[V]**
- Parakeet **encoder on CPU (ORT 1.29)**: int8 RTF ≈ 0.085 (8 threads) / fp32 ≈ 0.08. **[V]**
- **End-to-end** (mel CPU + encoder + TDT decode CPU): HTP path 65–120 ms per 5–8 s clip; CPU path
  540–830 ms. **[V]**
- **Accuracy** on 12 FLEURS clips (en/ru/es/uk): word-weighted WER ≈ 4.9 % HTP / 5.5 % CPU; ru & es
  0 %. Remaining errors are casing/number/punctuation normalization, not recognition. **[V]**
- **Silero VAD** on X2 CPU: 0.167 ms per 32 ms chunk (RTF 0.005), 1 thread. **[V]**
- macOS ANE (FluidAudio, published): encoder ~28 ms / 15 s window, RTFx ~128–146×. **[?] not ours**

Full tables and method: [`benchmarks.md`](benchmarks.md).

---

## 6. Recommended architecture (summary; full text in [`architecture.md`](architecture.md))

- **Rust core** owns audio, VAD, inference, text pipeline, platform I/O; **Tauri 2 + React/TS**
  shell is display + settings only. Audio never crosses the IPC boundary.
- A `SpeechEngine` trait with a `ParakeetEngine` that selects an `EncoderBackend`
  (`QnnHtp` → `OrtCpu` → later `CoreML`) at runtime and **reports the truth** in diagnostics.
- ONNX via `ort` `load-dynamic` against a shipped `onnxruntime.dll`; QNN via the plugin EP; mel and
  TDT greedy decode implemented in Rust.
- Silero VAD (MIT) as an independent stage over a shared 16 kHz ring buffer.
- Platform traits: `AudioCapture`, `GlobalHotkey`, `TextInjector`, `Clipboard`, `OverlayWindow`,
  `SystemTray`, `PlatformCapabilities` — Windows + macOS implemented, Linux stubbed.
- Model manager: HTTPS-only, SHA-256-pinned manifests, resumable, atomic, per-SoC artifact keys.

---

## 7. Rejected alternatives and why

| Rejected | Reason |
|---|---|
| **sherpa-onnx as the NPU backend** | Its QNN integration is `dlopen`-only (POSIX), C++-API-only (no C/Rust entry point), documented for Android only, and lists no Snapdragon-X SoC. Cannot reach our NPU without an upstream port. Kept as an alternative CPU engine. **[V]** |
| **Reusing OpenWritr's `encoder-model.bin`** | It is a V73/QAIRT-2.45 context binary; context binaries are SoC/arch-specific and forward-only. It will not load on V81. We generate our own. **[V]** |
| **`transcribe-rs` / `parakeet-rs` as dependencies** | `transcribe-rs` ignores the TDT duration head; `parakeet-rs` mis-places the Hann window and has no NPU path. We port and correct the code instead. **[V]** |
| **reqwest + rustls (aws-lc-rs)** | `aws-lc-sys` fails to assemble its ARM64 asm under the portable MSVC toolchain (`LNK1181 chacha-armv8.o`). Use native TLS (SChannel/Secure Transport). **[V]** |
| **Electron (OpenWhispr/Whispering)** | Heavier, no clean path to a native ARM64 NPU pipeline; sidecar model is a workaround for a problem Rust doesn't have. |
| **AI Hub offline compile as the v1 NPU path** | Requires a Qualcomm login/token and a 2.4 GB upload per model; on-device ORT prepare already works and needs neither. AI Hub kept as an optimization route for a smaller W8A16 artifact. **[V]** |
| **TEN VAD** | Apache-2.0 with an Agora non-compete rider; not OSI-clean; no Windows ARM64 prebuilt. Silero (MIT) instead. **[V]** |
| **DirectML EP for the encoder** | Available in pyke's static build, but no evidence it beats CPU on Adreno, and it competes with the NPU we already have working. Deferred. |

---

## 8. Open questions carried into implementation

1. On-device HTP prepare of the encoder takes ~2–3 min; can we ship a **pre-prepared V81 context
   binary** (generated on this machine, published under CC-BY-4.0) so users skip the wait? Context
   binaries are SoC-specific but should be reusable across identical X2 SoCs — to be confirmed on a
   second X2 machine.
2. INT8/INT16 QDQ vs fp16 on V81 — WER and memory trade-off (fp16 context is ~1.2 GB).
3. Best static window length on V81 (8 s vs 20 s vs 30 s) balancing prepare time, latency and chunk
   seams. 20 s already gives RTF 0.0026.
4. macOS: ORT CoreML EP (static fp16) vs a Swift/objc2 bridge to FluidAudio's `.mlmodelc` — must be
   measured on real Apple Silicon.
5. Whether current `ort` builders load EPContext wrappers cleanly (OpenWritr worked around a crash in
   rc.12 that was actually a missing-skel problem, now understood).

These are tracked in [`x2-npu.md`](x2-npu.md) §Experiments and in the FINAL_REPORT's "Remaining work".
