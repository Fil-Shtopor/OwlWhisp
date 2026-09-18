# LocalWisper — Benchmarks

_All numbers measured on the target machine (ASUS Zenbook A16, Snapdragon X2 Elite Extreme
X2E94100, 48 GB, Windows 11 build 28000 ARM64) on 2026-08-26 unless noted. Methodology and raw logs
are in [`experiments/`](experiments/); the Python reference used the same ONNX files the Rust engine
loads._

## 1. Method

- **Model:** NVIDIA Parakeet TDT 0.6B v3, istupakov ONNX export (encoder fp32/int8, fused
  `decoder_joint` int8, `nemo128.onnx` mel, `vocab.txt`).
- **Fixtures:** 15 FLEURS dev clips (CC-BY-4.0), 3 each for en / ru / es / uk / **zh**, 4–8 s,
  16 kHz mono. The Chinese three were added on 2026-09-18; every measurement recorded before that
  date was taken over the first twelve and says so in its own source line.
  Stored in `tests/fixtures/audio/` with reference transcripts in `fixtures.json`.
- **RTF** = processing wall-time ÷ audio duration (lower is faster; < 1 is faster than real time).
- **WER** = Levenshtein over lowercased, punctuation-stripped words; "word-weighted" averages by
  reference word count. Note FLEURS references keep punctuation/casing/spelled-out numbers, so a few
  points of WER are normalization, not recognition error.
- Timings exclude one-time model load and (for NPU) the one-time context-binary prepare.

### Words or characters

Accuracy is a Levenshtein error rate, and **the unit depends on the language**. Languages that
write spaces are scored by word (**WER**); Chinese, Cantonese, Japanese, Thai, Lao, Khmer, Burmese
and Tibetan are scored by character (**CER**), because they are written without word delimiters.
Korean is scored by word — it spaces its eojeol.

This is not a refinement, it is the difference between a metric and a coin flip. Against a Chinese
reference, a transcript with one wrong character and a transcript of unrelated nonsense both score
exactly **1.0** by word: the whole sentence is one token either way. `lw-core`'s test
`a_word_rate_is_meaningless_for_chinese_and_a_character_rate_is_not` pins both halves of that.

A run whose scored clips span both units has **no single total**. A word rate and a character rate
are different quantities and averaging them yields a number with no unit; `lw bench` prints the
per-language breakdown and says why instead.

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

### 5.0 A model is only run on the languages it claims

`lw bench` **skips** fixture clips whose language the engine does not declare in
`supported_languages()`. They are not transcribed, not timed, and not scored; the result prints how
many were skipped and in what languages.

This started as a scoring rule and became a running rule, in two steps.

**Scoring.** Measured on the X2:

| Model | scored as | WER |
|---|---|---:|
| Moonshine tiny **en** | 3 English clips (what it claims) | **0.092** |
| Moonshine tiny **en** | all 12 clips (en/ru/es/uk) | **0.850** |

The second figure is what you get by asking an English-only model to transcribe Russian. It is
arithmetically correct and descriptively worthless — worse than worthless in a catalog column
headed "accuracy", where it reads as *this model is bad* rather than *this model was asked the
wrong question*. Individual non-English clips exceed WER 1.0, because the model inserts more words
than the reference contains.

**Timing.** Excluding those clips from the WER was not enough, because they were still being
transcribed, and the time that took still landed in `cold_rtf`, `warm_rtf` and `audio_secs`. Nothing
downstream could tell it apart from real work. So `moonshine-tiny-en`'s speed was three English
clips plus nine clips of Spanish, Russian and Ukrainian it cannot speak — three quarters of its
measured RTF describing a job nobody would ever give it. Since 2026-09-18 those clips do not run at
all. `--include-unsupported` restores the old behaviour for the one case that wants it: asking what
a model actually does with a language it never advertised.

**Engines that declare nothing.** Several real exports carry no language metadata — a bare
`encoder/decoder/joiner` transducer directory says nothing about what it speaks — so
`supported_languages()` comes back empty and every clip counts again, by a different road. GigaAM v3
(Russian) and Parakeet TDT-CTC 110M (English) are both like this. The app falls back to the
catalog's language list for them, which is the same list it shows in the model row; `lw bench` has
no catalog, so it needs an explicit `--languages ru`.

The "over N clip(s) in <languages>" line printed with every result is the provenance that belongs in
any recorded measurement: a 3-clip English figure and a 12-clip multilingual figure must never be
compared as though they measured the same thing.

### 5.1 Measured `lw bench` results on the X2 (re-measured 2026-08-27, release ARM64 `lw.exe`)

Command (both tables): `lw bench tests/fixtures/audio --model-dir <dir> --backend {cpu,npu}`.

**CPU** (`--backend cpu`, ONNX Runtime 1.28.1 CPU EP, int8 encoder, default threads):

```
file                    dur(s) time(ms)     RTF  WER
fleurs_en_1.wav           6.00      203   0.034  0.05 [en]
fleurs_en_2.wav           7.50      224   0.030  0.09 [en]
fleurs_en_3.wav           7.92      295   0.037  0.05 [en]
fleurs_ru_1.wav           5.64      201   0.036  0.00 [ru]
fleurs_ru_2.wav           6.96      197   0.028  0.15 [ru]
fleurs_ru_3.wav           7.50      206   0.027  0.00 [ru]
fleurs_es_1.wav           7.26      210   0.029  0.07 [es]
fleurs_es_2.wav           7.14      203   0.028  0.00 [es]
fleurs_es_3.wav           7.74      218   0.028  0.00 [es]
fleurs_uk_1.wav           5.28      248   0.047  0.12 [uk]
fleurs_uk_2.wav           5.10      181   0.036  0.00 [uk]
fleurs_uk_3.wav           7.80      219   0.028  0.08 [uk]
---
mean RTF: 0.0324   word-weighted WER: 0.054
```

**NPU** (`--backend npu`, encoder on Hexagon HTP V81, cached context binary):

```
file                    dur(s) time(ms)     RTF  WER
fleurs_en_1.wav           6.00      113   0.019  0.20 [en]
fleurs_en_2.wav           7.50      128   0.017  0.00 [en]
fleurs_en_3.wav           7.92      109   0.014  0.09 [en]
fleurs_ru_1.wav           5.64      122   0.022  0.00 [ru]
fleurs_ru_2.wav           6.96      102   0.015  0.00 [ru]
fleurs_ru_3.wav           7.50      112   0.015  0.00 [ru]
fleurs_es_1.wav           7.26       76   0.010  0.00 [es]
fleurs_es_2.wav           7.14       94   0.013  0.00 [es]
fleurs_es_3.wav           7.74      144   0.019  0.00 [es]
fleurs_uk_1.wav           5.28       98   0.019  0.12 [uk]
fleurs_uk_2.wav           5.10       99   0.019  0.00 [uk]
fleurs_uk_3.wav           7.80       88   0.011  0.08 [uk]
---
mean RTF: 0.0160   word-weighted WER: 0.048
```

RTF is wall-clock and varies run to run with scheduling and thermals: three consecutive CPU
runs of the table above gave mean RTF 0.0312 / 0.0324 / 0.0346, and the NPU path has been observed
between 0.0145 and 0.0160. WER, by contrast, is deterministic for a given build and model set.

**GPU** (`--backend webgpu`, WebGPU plugin EP → Dawn → D3D12 on the integrated Adreno). This run
used a locally produced **static fp32** encoder; see the note below for the shipped artifact:

```
file                    dur(s) time(ms)     RTF  WER
fleurs_en_1.wav           6.00      871   0.145  0.20 [en]
fleurs_en_2.wav           7.50      347   0.046  0.00 [en]
fleurs_en_3.wav           7.92      365   0.046  0.09 [en]
fleurs_ru_1.wav           5.64      350   0.062  0.00 [ru]
fleurs_ru_2.wav           6.96      384   0.055  0.00 [ru]
fleurs_ru_3.wav           7.50      360   0.048  0.00 [ru]
fleurs_es_1.wav           7.26      345   0.048  0.00 [es]
fleurs_es_2.wav           7.14      369   0.052  0.00 [es]
fleurs_es_3.wav           7.74      355   0.046  0.00 [es]
fleurs_uk_1.wav           5.28      346   0.065  0.12 [uk]
fleurs_uk_2.wav           5.10      366   0.072  0.00 [uk]
fleurs_uk_3.wav           7.80      359   0.046  0.17 [uk]
---
mean RTF: 0.0609   word-weighted WER: 0.054
```

The first clip carries the one-time shader compilation (871 ms vs ~360 ms steady state), which is
why the cold/warm split matters more here than on the other two paths.

**On the shipped artifact set** — the same quantized encoder the CPU path uses, which is all a
standard install downloads — the same GPU measures **mean RTF 0.0862, word-weighted WER 0.048**.
That is the number that describes a fresh install: GPU acceleration needs no extra download. The
engine prefers a dynamic fp32 encoder, then a static one, then the quantized one, and its
backend-selection notes say which it took.

Ordering on **this** machine is NPU (0.0160) → CPU (0.0324) → GPU (0.0609). That is not a general
claim about GPUs: an 18-core Oryon competing against an integrated mobile Adreno is close to the
worst case for the GPU row. A discrete GPU is expected to land well ahead of the CPU — expected,
not measured, because no such machine was available.

> **Correction (2026-08-27).** An earlier revision of this section recorded the CPU run as
> **RTF 0.032 / WER 0.042**, with three of the twelve rows collapsed into a `fleurs_es_1/2/3` summary
> line. The RTF reproduces; the **WER does not**. Re-running the same command against the same model
> files and the same staged runtime now yields **0.054**, deterministically — three consecutive
> release runs and one debug run all returned 0.054, and `git diff` shows the mel frontend, encoder,
> TDT decoder and audio path are byte-for-byte unchanged (formatting only) since the commit that
> recorded the original figure. The two rows that differ are `fleurs_en_2` (0.04 → 0.09) and
> `fleurs_es_1` (recorded as 0.00 inside the collapsed row → 0.07); the other ten match exactly, and
> the NPU table above reproduces its 0.048 row-for-row. **The provenance of the 0.042 figure could
> not be established, so it is withdrawn** — 0.054 is the number the current, committed code
> produces, and it is the one every downstream document now cites.
>
> Note that CPU WER (0.054) is *worse* than NPU WER (0.048) here. That is not an anomaly: the CPU
> path runs an **int8-quantized** encoder while the HTP path runs **fp16**, so the accelerated path
> is the more numerically faithful one on this machine.

These are the **actual Rust engine** numbers (not the Python reference). The Rust engine loads the
identical ONNX files. End-to-end totals include mel (CPU) + encoder + TDT decode (CPU); on the NPU
path the ~90–110 ms is dominated by mel + per-frame TDT decode on the CPU, while the encoder itself
is ~23 ms on the HTP (§2). The small WER differences between CPU and NPU (and the en_1 0.20 outlier)
are number-word / casing normalization and fp16-vs-int8 decode ties, not recognition failures — the
transcripts are correct sentences. First NPU run adds a one-time ~106 s context prepare (cached
thereafter as a 1.2 GB `*_qnn.bin` that reloads in ~2 s).

## 6. Estimates vs measurements (`lw models` / `lw bench --quick`)

Everything in §§1–5 is **measured**. The model catalog also has to say something useful about models
that are *not installed yet* — you cannot benchmark a model you have not downloaded. Those numbers
are **estimates**, and the code, the JSON and the tables keep the two apart on purpose. The rule:

> A number is a **measurement** only if it was produced by running the model, and it is reported
> together with the machine it ran on. Everything else is an **estimate** and is marked with `~`
> under a column header containing `EST`.

### 6.1 The estimate heuristic

`lw_core::model::catalog::estimate_rtf(speed_tier, hardware, cpu_cores)` — deliberately simple, so
you can judge it:

1. Each catalog entry carries an editorial **speed tier**. Each tier maps to a base RTF on a
   documented reference machine (an 8-performance-core ARM64 laptop CPU on the ORT CPU EP):

   | Speed tier | Base RTF @ 8 cores |
   |---|---|
   | `slow` | 0.60 |
   | `moderate` | 0.25 |
   | `fast` | 0.08 |
   | `very_fast` | 0.035 |

   The `very_fast` anchor is set from the measured Parakeet CPU RTF of 0.032 in §5.1. The other
   three are ordinal steps away from it — they are **not** measurements of anything.

2. Scale by core count: `clamp(8 / clamp(cores, 2, 16), 0.75, 4.0)`. The clamps are asymmetric on
   purpose. A small or unidentified machine takes up to a 4× penalty; a big machine is credited
   with at most 1.33×, because only the encoder is multi-threaded — the mel front end and the
   per-frame TDT decode are not. (Evidence: the 18-core X2 measured 0.032, essentially the 8-core
   reference figure.)

3. If an accelerator is available *and* the entry ships an artifact for it, multiply by
   `NPU_RTF_RATIO = 0.45` (Qualcomm) or `COREML_RTF_RATIO = 0.60` (Apple). The NPU ratio is the one
   measured pair we have — 0.0145 / 0.032 from §5.1 — and is validated for **no other model**. The
   CoreML ratio is an unvalidated placeholder; see §7.

An accelerator counts only when its *execution provider* is really available, not merely when the
OS reports the hardware: `lw-platform`'s detector can see a Hexagon driver package while the QNN EP
fails to load. `lw models list` / `info` register the QNN EP and enumerate devices; `lw models
compare` and `lw bench --quick` use the weaker "is the EP library there" check instead, because
registering the EP behind an engine's back pulls it into sessions meant to stay on the CPU.

**How good is the heuristic?** On the one machine where both numbers exist, `lw models list`
estimated **~0.0118** for Parakeet on the NPU and `lw models compare` measured **0.0142** on the
same machine minutes later — about 20 % optimistic. That is the accuracy class to expect: right
order of magnitude, useful for ranking, useless as a promise.

### 6.2 `lw bench --quick`

A few-second self-measurement with per-stage timings that needs no fixtures directory:

```bash
lw bench --quick --backend cpu --model-dir <models>/parakeet-tdt-0.6b-v3
```

It reuses `tests/fixtures/audio` when it can find one (walking up from the working directory and
from the executable, or `$LW_FIXTURES`), and only synthesizes audio when it cannot. Synthetic audio
is announced loudly, because the TDT decoder emits far fewer tokens on a synthetic sweep than on
speech, which makes the RTF a **lower bound** rather than a representative figure. Measured on the
X2 (2026-08-27, real FLEURS clips, CPU EP):

```
stage (measured)                     time
ort runtime init                      1 ms
engine load (sessions)             1171 ms
health probe                         60 ms   provider: ONNX Runtime CPU
transcribe fleurs_en_1.wav          185 ms   RTF 0.0308   WER 0.05
transcribe fleurs_en_2.wav          197 ms   RTF 0.0262   WER 0.09
transcribe fleurs_en_3.wav          209 ms   RTF 0.0264   WER 0.05

MEASURED RTF (first/cold run) : 0.0308
MEASURED RTF (warm mean)      : 0.0263   over 2 run(s), 197–209 ms
MEASURED WER (word-weighted)  : 0.062   over 3 clip(s)
```

The cold run is reported separately from the warm mean because the first inference pays lazy
allocation and cache warm-up. The warm mean, 0.0263, is in line with the full 12-clip fixture run
in §5.1 (0.032) — the quick bench is a smaller, English-only sample, not a different measurement.
The WER here (0.062) is higher than the 12-clip word-weighted figure precisely because it is only
the three English clips, which carry all of the number-word/casing normalization noise (§3).

The fixture-based `lw bench <dir>` behaviour is unchanged; `--quick` is purely additive.

### 6.3 `lw models compare`

One table, estimates and measurements in separate columns, for every catalog entry:

```
ID                     EST HW     EST RTF  MEAS HW  MEAS RTF  MEAS WER  STATUS
parakeet-tdt-0.6b-v3   qnn-npu    ~0.0118  qnn-npu    0.0142     0.092  measured just now
whisper-base           cpu        ~0.0600        —         —         —  needs the `sherpa` engine feature …
```

- **EST HW / EST RTF** — the estimate and the target it assumes.
- **MEAS HW / MEAS RTF / MEAS WER** — a real run performed by this command, on hardware read back
  from the engine *after* initialization. An engine that asked for the NPU and fell back reports
  `cpu` here, and the status column says the two columns are then not comparable.
- Models that are not installed, have no pinned manifest, or need an engine this build lacks show
  `—` in the measured columns with the reason spelled out. Nothing is ever extrapolated into them.
- Below the table, `compare` lists the catalog's own reference measurements together with the
  machine each was taken on, so a number measured elsewhere can never be mistaken for yours.

`--fixtures <dir>` measures with real reference transcripts (enabling WER), `--backend` forces a
target, `--no-run` shows estimates only, and `--json` emits the same data machine-readably with an
explicit `estimated_rtf_is_an_estimate: true` flag on every row.

### 6.4 What the catalog does *not* claim

`models/catalog.json` carries verified sizes and URLs for the Whisper and Moonshine entries (read
from the GitHub release API for `k2-fsa/sherpa-onnx` tag `asr-models`), but **no** SHA-256 hashes,
because none are published and we have not hashed the archives ourselves. Consequently those
entries have no pinned manifest and `lw models install` refuses them rather than downloading
something unverified. Their `wer_estimates` are empty and their `disk_bytes` is `null` for the same
reason: an honest blank instead of a plausible fabrication. Every entry's `notes` field states
explicitly what is verified and what is not.

## 7. macOS / Linux (not measured here)

- macOS ANE (FluidAudio, published): encoder ~28 ms / 15 s window on M5, RTFx ~128–146×. **Not
  measured on our hardware** — we develop on Windows ARM64; the macOS CoreML path must be benchmarked
  on real Apple Silicon before any acceleration claim.
- Linux: ORT CPU int8, expected RTF comparable to the Windows CPU path on similar ARM cores.

## 8. Memory (approximate, from process inspection)

- CPU int8 encoder resident: ~1–1.3 GB during inference (int8 weights + fp32 activations).
- HTP fp16 context: ~1.2 GB binary; NPU-side memory is managed by the HTP.
- Silero VAD: a few MB.

## 9. Benchmark matrix status

| Config | Latency | RTF | WER | Measured? |
|---|---|---|---|---|
| Parakeet CPU (X2) | ✅ | ✅ (0.032) | ✅ (5.4%) | **yes — via `lw bench`** |
| Parakeet QNN/NPU (X2 V81) | ✅ | ✅ (0.0145) | ✅ (4.8%) | **yes — via `lw bench`** |
| Whisper CPU | — | — | — | not in v1 (engine adapter stub) |
| Whisper QNN | — | — | — | out of scope for v1 |
| macOS CoreML/ANE | — | — | — | not on our hardware |
