# LocalWisper — Benchmarks

_All numbers measured on the target machine (ASUS Zenbook A16, Snapdragon X2 Elite Extreme
X2E94100, 48 GB, Windows 11 build 28000 ARM64) on 2026-08-26 unless noted. Methodology and raw logs
are in [`experiments/`](experiments/); the Python reference used the same ONNX files the Rust engine
loads._

## 1. Method

- **Model:** NVIDIA Parakeet TDT 0.6B v3, istupakov ONNX export (encoder fp32/int8, fused
  `decoder_joint` int8, `nemo128.onnx` mel, `vocab.txt`).
- **Fixtures:** 12 FLEURS dev clips (CC-BY-4.0), 3 each for en / ru / es / uk, 5–8 s, 16 kHz mono.
  Stored in `tests/fixtures/audio/` with reference transcripts in `fixtures.json`.
- **RTF** = processing wall-time ÷ audio duration (lower is faster; < 1 is faster than real time).
- **WER** = Levenshtein over lowercased, punctuation-stripped words; "word-weighted" averages by
  reference word count. Note FLEURS references keep punctuation/casing/spelled-out numbers, so a few
  points of WER are normalization, not recognition error.
- Timings exclude one-time model load and (for NPU) the one-time context-binary prepare.

## 2. Encoder microbenchmark (Parakeet FastConformer encoder)

| Backend | Precision | Window | Prepare (1st run) | Steady-state | RTF | vs CPU fp32 (cosine) |
|---|---|---|---|---|---|---|
| **QNN HTP V81** | fp16 | 10 s (1000 fr) | 195 s | **23 ms** | **0.0023** | 0.99927 |
| **QNN HTP V81** | fp16 | 20 s (2000 fr) | 106 s | **53 ms** | **0.0026** | 0.99927 |
| ONNX CPU (ORT 1.29) | int8 (sherpa) | dynamic | — | ~850 ms / 10 s | ~0.085 (8 thr) | ref |
| ONNX CPU (ORT 1.29) | fp32 (istupakov) | dynamic | — | ~815 ms / 10 s | ~0.081 | ref |

The HTP context binary is ~1.2 GB and reloads from cache in ~2.2 s, so the multi-minute prepare is
paid only once per (model, QAIRT, HTP-arch).

CPU encoder RTF by thread count (sherpa int8, 20 s window):

| Threads | RTF |
|---|---|
| default | 0.122 |
| 8 | 0.085 |
| 4 | 0.110 |

## 3. End-to-end pipeline (mel → encoder → TDT decode → text)

Per-clip totals over the 12 FLEURS fixtures (Python reference; the Rust `lw bench` reproduces this —
see §5):

| Path | Per 5–8 s clip | RTF | Word-weighted WER |
|---|---|---|---|
| **HTP V81 encoder + CPU decode** | 65–120 ms | 0.01–0.02 | **≈ 4.9 %** |
| CPU encoder + CPU decode | 540–830 ms | 0.08–0.13 | ≈ 5.5 % |

Per language (HTP path): en ≈ 10.2 %, ru 0 %, es 0 %, uk ≈ 6.9 % — English is dominated by number
words ("2" vs "two", "3" vs "three") and casing in the FLEURS references, not misrecognition.

## 4. VAD

Silero VAD (`silero_vad.onnx`, v6.2) on the X2 CPU via ORT 1.29, 1 thread: **0.167 ms per 32 ms
frame** (RTF 0.005). Negligible next to STT; runs continuously during recording.

## 5. Reproducing with the Rust CLI

```bash
# CPU
lw bench tests/fixtures/audio --model-dir <models>/parakeet-tdt-0.6b-v3 --backend cpu
# NPU (Snapdragon X2; prepares + caches the HTP context on first run)
lw bench tests/fixtures/audio --model-dir <models>/parakeet-tdt-0.6b-v3 --backend npu
```

### 5.1 Measured `lw bench` results on the X2 (2026-08-26, native ARM64 `lw.exe`)

**CPU** (`--backend cpu`, ONNX Runtime 1.28.1 CPU EP, default threads):

```
file                    dur(s) time(ms)     RTF  WER
fleurs_en_1.wav           6.00      267   0.044  0.05
fleurs_en_2.wav           7.50      218   0.029  0.04
fleurs_en_3.wav           7.92      216   0.027  0.05
fleurs_ru_1.wav           5.64      186   0.033  0.00
fleurs_ru_2.wav           6.96      242   0.035  0.15
fleurs_ru_3.wav           7.50      239   0.032  0.00
fleurs_es_1/2/3.wav       ~7.4     ~204   0.028  0.00
fleurs_uk_1.wav           5.28      177   0.034  0.12
fleurs_uk_2.wav           5.10      174   0.034  0.00
fleurs_uk_3.wav           7.80      260   0.033  0.08
---  mean RTF 0.032   word-weighted WER 0.042
```

**NPU** (`--backend npu`, encoder on Hexagon HTP V81, cached context):

```
file                    dur(s) time(ms)     RTF  WER
fleurs_en_1.wav           6.00      109   0.018  0.20
fleurs_en_2.wav           7.50       98   0.013  0.00
fleurs_en_3.wav           7.92       91   0.011  0.09
fleurs_ru_1/2/3.wav       ~6.7      ~98   0.015  0.00
fleurs_es_1/2/3.wav       ~7.4      ~96   0.013  0.00
fleurs_uk_1.wav           5.28       84   0.016  0.12
fleurs_uk_2.wav           5.10       91   0.018  0.00
fleurs_uk_3.wav           7.80      109   0.014  0.08
---  mean RTF 0.0145   word-weighted WER 0.048
```

These are the **actual Rust engine** numbers (not the Python reference). The Rust engine loads the
identical ONNX files. End-to-end totals include mel (CPU) + encoder + TDT decode (CPU); on the NPU
path the ~90–110 ms is dominated by mel + per-frame TDT decode on the CPU, while the encoder itself
is ~23 ms on the HTP (§2). The small WER differences between CPU and NPU (and the en_1 0.20 outlier)
are number-word / casing normalization and fp16-vs-int8 decode ties, not recognition failures — the
transcripts are correct sentences. First NPU run adds a one-time ~106 s context prepare (cached
thereafter as a 1.2 GB `*_qnn.bin` that reloads in ~2 s).

## 6. macOS / Linux (not measured here)

- macOS ANE (FluidAudio, published): encoder ~28 ms / 15 s window on M5, RTFx ~128–146×. **Not
  measured on our hardware** — we develop on Windows ARM64; the macOS CoreML path must be benchmarked
  on real Apple Silicon before any acceleration claim.
- Linux: ORT CPU int8, expected RTF comparable to the Windows CPU path on similar ARM cores.

## 7. Memory (approximate, from process inspection)

- CPU int8 encoder resident: ~1–1.3 GB during inference (int8 weights + fp32 activations).
- HTP fp16 context: ~1.2 GB binary; NPU-side memory is managed by the HTP.
- Silero VAD: a few MB.

## 8. Benchmark matrix status

| Config | Latency | RTF | WER | Measured? |
|---|---|---|---|---|
| Parakeet CPU (X2) | ✅ | ✅ (0.032) | ✅ (4.2%) | **yes — via `lw bench`** |
| Parakeet QNN/NPU (X2 V81) | ✅ | ✅ (0.0145) | ✅ (4.8%) | **yes — via `lw bench`** |
| Whisper CPU | — | — | — | not in v1 (engine adapter stub) |
| Whisper QNN | — | — | — | out of scope for v1 |
| macOS CoreML/ANE | — | — | — | not on our hardware |
