# Snapdragon X2 Elite NPU — investigation & procedure

_This is the record of the project's most important research/engineering goal: getting **NVIDIA
Parakeet TDT 0.6B v3** to run on the **Snapdragon X2 Elite Extreme (X2E94100 / SC8480XP)** Hexagon
NPU. Everything marked **[V]** was executed and observed on the target machine on 2026-08-26._

## 1. What the X2 Elite NPU is

| Property | Value | Source |
|---|---|---|
| SoC | Snapdragon X2 Elite Extreme, part **X2E94100**, chip **SC8480XP** | local `Win32_Processor`, product brief **[V]** |
| Hexagon HTP arch | **V81** | DriverStore `libQnnHtpV81SkelDrv.so` / `QnnHtpV81StubDrv.dll`; `ai-hub-models` `devices_and_chipsets.yaml` (`htp_version 81`); `onnxruntime-qnn` `soc_utils.cc` **[V]** |
| QNN `soc_model` id | **88** | `soc_utils.cc` `{"SC8480XP", 88}` **[V]** |
| NPU driver | 30.0.220.11010 (2026-01-26), package `qcnspmcdm8480`, QNN libs v2.41.0 | local `Get-PnpDevice` / DriverStore **[V]** |
| INT8 throughput | ~80 TOPS (this SKU) | Qualcomm product brief |
| PnP id | `ACPI\VEN_QCOM&DEV_0FF0&SUBSYS_CRD08480` (CRD = Compute Reference Device) | local **[V]** |

The device driver ships the QNN HTP **V81** stub, skel and prepare libraries — i.e. the machine can
run and *prepare* QNN graphs for its own SoC without any external SDK.

## 2. Why OpenWritr's artifact cannot be reused

OpenWritr publishes a Parakeet encoder as a **QNN context binary** compiled via Qualcomm AI Hub for
**Snapdragon X Elite (HTP V73, QAIRT 2.45)**. QNN context binaries are **SoC/arch-specific and
forward-only in QAIRT version** (Qualcomm's own FAQ). A V73 binary will not load on V81. Its
device gate is also a CPU-name substring (`"x elite"`/`"x1e"`) that *rejects* the X2's
`"Snapdragon(R) X2 Elite Extreme"` string. So the X2 path had to be established from scratch. **[V]**

## 3. The runtime that reaches the NPU (no login, no vendor artifact)

`pip install onnxruntime-qnn` gives Qualcomm's **plugin QNN execution provider** (MIT for the EP,
Qualcomm "AI Stack License" for the QNN DLLs). Version **2.5.0** bundles **QAIRT 2.49.40** and, under
`libs/arm64ec/`, ships the **V81** stub+skel+cat (`QnnHtpV81Stub.dll`, `libQnnHtpV81Skel.so`,
`libqnnhtpv81.cat`) plus `QnnHtp.dll`, `QnnSystem.dll`, `QnnHtpPrepare.dll`,
`onnxruntime_providers_qnn.dll`. Those DLLs are **ARM64X** — they load into a **native ARM64**
process. **[V]**

The plugin registers into a stock `onnxruntime` (1.29.0 verified) via the plugin-EP C API:

```
RegisterExecutionProviderLibrary("QNNExecutionProvider", onnxruntime_providers_qnn.dll)
  → GetEpDevices()  (enumerates an NPU OrtEpDevice, vendor Qualcomm)
  → SessionOptionsAppendExecutionProvider_V2(npu_devices, {backend_type=htp, ...})
```

The `ort` Rust crate exposes all of this (`Environment::register_ep_library`, `EpDevice`,
`SessionBuilder::with_devices` / `with_auto_device(AutoDevicePolicy::PreferNpu)`), so a native Rust
app can drive it without the legacy compiled-in EP. **[V]**

## 4. The experiment (reproducible)

Scripts live in `scripts/qnn/` and `scripts/benchmarks/`; the raw logs are in
`docs/experiments/`.

1. **Export**: use the public `istupakov/parakeet-tdt-0.6b-v3-onnx` fp32 encoder
   (`encoder-model.onnx` + `.data`, 2.44 GB), decoder `decoder_joint-model.int8.onnx`, mel
   preprocessor `nemo128.onnx`, `vocab.txt`. (CC-BY-4.0.) **[V]**
2. **Static shape**: fix `audio_signal` to `[1, 128, T]` and `length` to `[1]` for a chosen window
   `T` (frames = seconds × 100); constant-fold the dynamic attention-mask chain (`Shape→Gather→
   Range→Expand`) so the graph is fully static. ORT's basic constant-folding reduces the encoder to
   ~1458 nodes. (`scripts/qnn/probe_encoder_htp.py`.) **[V]**
3. **Run on HTP**: create an ORT session with the QNN plugin EP, `backend_type=htp`,
   `htp_performance_mode=burst`, `htp_graph_finalization_optimization_mode=3`,
   `enable_htp_fp16_precision=1` (fp16, **no** quantization/calibration needed). ORT's on-device
   `QnnHtpPrepare.dll` compiles the whole encoder to **one EPContext node**. **[V]**
4. **Cache the context binary**: pass `ep.context_enable=1`, `ep.context_file_path=<...>_ctx.onnx`,
   `ep.context_embed_mode=0` → ORT writes a tiny `_ctx.onnx` + a `~1.2 GB` `*_qnn.bin`. Reloading
   from that binary skips the long prepare (~2.2 s vs ~195 s). **[V]**
5. **Decode**: run `nemo128.onnx` (mel) and the fused `decoder_joint` on the CPU EP, with the TDT
   greedy loop (split 8193 token + 5 duration logits; argmax each; advance LSTM state only on
   non-blank; skip `duration` frames; ≤ 10 symbols/frame). (`scripts/benchmarks/tdt_reference.py`.) **[V]**

### Observed results **[V]**

| Window `T` | Prepare (first run) | Context `.bin` | Steady-state inference | RTF | Cosine vs CPU fp32 |
|---|---|---|---|---|---|
| 1000 frames (10 s) | 195 s | 1.20 GB | 23 ms | 0.0023 | 0.99927 |
| 2000 frames (20 s) | 106 s | 1.22 GB | 53 ms | 0.0026 | 0.99927 |

End-to-end over 12 FLEURS clips (en/ru/es/uk), HTP encoder path: **65–120 ms per 5–8 s clip**,
word-weighted **WER ≈ 4.9 %** (ru & es 0 %; remaining errors are casing/number normalization). CPU
path: 540–830 ms per clip. See [`benchmarks.md`](benchmarks.md).

One benign warning appears during context creation: `Failed to get compatibility info. Unknown HTP
arch 0`. Root cause (from the ORT source): on ARM64 the EP reads the HTP arch from
`QnnDevice_getPlatformInfo`; when that returns 0 and the in-box user-mode-driver path is present, the
compatibility string is left empty. It does **not** affect execution — the graph still runs on the
HTP — but it means our cached context carries no compatibility string, so on load we treat a
`NotApplicable` compatibility result as "attempt the load" rather than trusting it. **[V]**

## 5. How this maps into the app

`lw-ort::QnnProvider` + `lw-engine-parakeet::QnnHtpEncoder` implement steps 3–4 from Rust:

- Ship a stock `onnxruntime.dll` and the `onnxruntime-qnn` V81 DLL set beside the exe (bundled in
  the installer per the Qualcomm licence — never a standalone download).
- On first use with an NPU present, prepare the encoder context on-device and cache the `_ctx.onnx`
  + `_qnn.bin` in `%LOCALAPPDATA%/LocalWisper/cache`, keyed by
  `(model hash, ORT version, QAIRT version, HTP arch)`. Subsequent launches reload in ~2 s.
- Report the truth: `Provider::QnnHtp`, `Acceleration::Npu`, device `"Snapdragon X2 Elite HTP
  (V81)"`. If prepare or load fails, fall back to `OrtCpuEncoder` and report `Provider::OnnxCpu`.

Recommended session options (from the ORT-QNN dossier): `backend_type=htp`,
`enable_htp_fp16_precision=1`, `htp_performance_mode=burst`,
`htp_graph_finalization_optimization_mode=3`, and consider `enable_htp_fp16_clamp_overflow=1` on
V81. Runtime tuple: `onnxruntime` 1.28.1/1.29.0 + `onnxruntime-qnn` 2.5.0 (QAIRT 2.49.40) + `ort`
2.0.0-rc.13 with `load-dynamic`.

## 6. What is verified vs. what remains

**Verified [V]:** the full Parakeet encoder runs on the X2 Elite HTP V81 via ORT QNN EP with fp16
online prepare; correctness (cosine 0.9993, WER ≈ 5 %); context-binary caching and reload; the
runtime is redistributable (MIT EP + Qualcomm object-code-only licence).

**Not yet verified [?]:**
- Whether a context binary prepared on this X2 loads on *another* X2 machine (SoC-specific but should
  be reusable across identical SoCs) — needs a second X2.
- INT8-weight/INT16-activation QDQ vs fp16 on V81: WER and the ~1.2 GB fp16 context size trade-off.
  The QDQ script (`scripts/qnn/quantize_encoder_qdq.py`) is written; naïve per-channel QDQ hit HTP
  op-config errors (6020 / LayerNorm 3110) and needs per-tensor + uint16 activations.
- Whether `enable_htp_graph_splitting=1` (QAIRT 2.49+) shortens the ~2–3 min prepare.
- Optimal window length on V81 (8 / 20 / 30 s) trading prepare time vs. chunk seams.
- Whether the ORT AI-Hub offline route (submit_compile + link) yields a smaller W8A16 artifact worth
  publishing so users skip on-device prepare entirely (`scripts/qnn/aihub_compile_encoder.py` stub).

## 7. Fallback guarantee

If the NPU path is unavailable for any reason (no NPU, prepare failure, missing DLLs, incompatible
driver), the app runs Parakeet on the **CPU int8** encoder (RTF ≈ 0.08–0.13 on the Oryon cores) and
the diagnostics/overlay report `X2 NPU backend unavailable — using Parakeet CPU`. The NPU backend can
be added or upgraded later without any UI change, because selection is behind the
`EncoderBackend`/`SpeechEngine` traits.
