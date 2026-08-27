# LocalWisper — Choosing a model

_How to see what exists, decide what suits your machine **before** downloading a gigabyte, and get
real numbers for your own hardware **after**._

The catalog lives in [`models/catalog.json`](../models/catalog.json) (schema v1) and is compiled
into the binary, so `lw models` works without a repo checkout. The data model and the recommender
are in `lw-core/src/model/catalog.rs`.

## 1. The one rule

> **Estimates are estimates.** A number is a *measurement* only when it came from actually running
> the model, and only when it is reported together with the machine it ran on. Everything else is
> an estimate, printed with a `~` under a column whose header contains `EST`.

`lw` never promotes an estimate to a measurement, never fills a blank with a plausible guess, and
never presents a number measured on somebody else's machine without naming that machine. Where a
figure could not be verified — a SHA-256 that upstream does not publish, a WER we could not source
— the field is left empty and the entry's `notes` says so.

## 2. Browse the catalog

```bash
lw models list                       # table, marks the pick for your hardware
lw models list --json                # same data, machine-readable
lw models list --dest <models-dir>   # also report what is already installed there
```

```
   ID                    NAME                            ENGINE          DOWNLOAD LANGS SPEED      QUALITY   EST RTF INSTALLED
*  parakeet-tdt-0.6b-v3  NVIDIA Parakeet TDT 0.6B v3     parakeet_tdt   639.6 MiB    25 very-fast  best       ~0.012 yes
   whisper-small         Whisper small (sherpa-onnx exp… whisper        609.8 MiB    99 moderate   better     ~0.188 n/a
                         └─ needs the `sherpa` engine feature, which this build does not include
```

- `*` marks the top-ranked entry this build can actually run here. Ranking is: runnable first, then
  higher quality tier, then lower estimated RTF, then id.
- `EST RTF` is an estimate for **your** detected hardware — see
  [benchmarks.md §6.1](benchmarks.md#61-the-estimate-heuristic) for the exact arithmetic.
- `INSTALLED` is `yes` / `partial` / `no` from the pinned manifest, or `n/a` when an entry has no
  pinned file set at all.
- A `└─` line under a row lists what stands between your machine and that model.

`lw models info <id>` prints everything: full language list, licence, upstream URL, the estimate
with its disclaimer, every measurement in the catalog with its machine and citation, and the
entry's honest `notes`.

## 3. Which one should you use?

**On a Snapdragon X / X2 laptop, or any machine at all: `parakeet-tdt-0.6b-v3`.** It is the only
model with a working engine in this build, the only one with a Qualcomm NPU path, and the only one
with measured numbers on real hardware (RTF 0.032 CPU / 0.0145 NPU, word-weighted WER ≈ 4.8–5.4 % —
[§5.1](benchmarks.md#51-measured-lw-bench-results-on-the-x2-2026-08-26-native-arm64-lwexe)). It
covers 25 European languages with punctuation and casing. Cost: a 640 MiB download.

The other entries are catalogued so you can see what a `sherpa`-backed build would offer, and are
marked `requires_engine_feature: "sherpa"`:

| If you need… | Look at | Why |
|---|---|---|
| a language Parakeet does not cover | `whisper-base` (99 languages) | broad coverage, modest accuracy |
| more accuracy than `base`, same coverage | `whisper-small` | ~3× the compute of `base` |
| the smallest English-only footprint | `whisper-tiny-en` or `moonshine-tiny-en` | ~110 MiB downloads |
| short utterances / low latency, English | `moonshine-tiny-en` | no fixed 30 s padding, unlike Whisper |

Rules of thumb the catalog encodes: **quality tier** is an editorial ranking of the model family,
not a measurement; **speed tier** is the input to the RTF estimate; and neither tells you how the
model does on *your* audio, accent, or vocabulary. The only way to know that is §5.

## 4. Install

```bash
lw models install parakeet-tdt-0.6b-v3 --dest "$LOCALAPPDATA/LocalWisper/models"
```

The install resolves the entry's manifest under `models/manifests/`, picks the best artifact set
for the detected hardware (`--target` overrides), and downloads through
`lw_core::model::ModelDownloader`: HTTPS only, resumable via HTTP `Range`, every file size- and
SHA-256-checked against the committed manifest, staged in a temporary directory and promoted with
an atomic rename so an interrupted install never leaves a half-populated model directory. No
downloaded file is ever executed.

If an entry has **no pinned manifest**, install refuses:

```
error: `whisper-base` has no pinned manifest, so there is nothing safe to download.
Its file set has not been hash-pinned yet (see `lw models info whisper-base`); LocalWisper refuses
to fetch unverified model files. A manifest is added once the files are downloaded, hashed and
committed under models/manifests/.
```

That is deliberate. The Whisper and Moonshine archives have verified sizes and URLs (read from the
GitHub release API for `k2-fsa/sherpa-onnx` tag `asr-models`) but upstream publishes no SHA-256, so
pinning them honestly requires downloading and hashing them first. Until then the CLI declines
rather than fetching something it cannot verify.

## 5. Measure your own machine

Estimates get you to a shortlist. These get you the truth:

```bash
# a few seconds, per-stage timings, no fixtures directory needed
lw bench --quick --backend cpu --model-dir <models>/parakeet-tdt-0.6b-v3

# every catalog entry: estimate next to a measurement taken right now
lw models compare --dest <models-dir>

# the full 12-clip fixture benchmark (RTF + WER)
lw bench tests/fixtures/audio --model-dir <models>/parakeet-tdt-0.6b-v3 --backend npu
```

`lw bench --quick` reports the ORT init, model load, health probe, cold run and warm mean
separately, so you can tell a slow model from a slow first inference. It prefers real speech from
`tests/fixtures/audio` and says so loudly when it has to fall back to synthesized audio (on which
RTF is a lower bound, not a representative figure).

`lw models compare` puts `EST HW` / `EST RTF` next to `MEAS HW` / `MEAS RTF` / `MEAS WER` and reads
the measured hardware back from the engine *after* initialization — so an engine that asked for the
NPU and fell back to the CPU is reported as `cpu`, with the status column pointing out that the two
columns then describe different hardware.

For the full worked example, the heuristic's arithmetic, and how close it came on real hardware,
see [benchmarks.md §6](benchmarks.md#6-estimates-vs-measurements-lw-models--lw-bench---quick).

## 6. Adding a model to the catalog

1. Add an entry to `models/catalog.json`. Required: `id`, `name`, `engine`, `description`,
   `languages`, `quality`, `speed`, `hardware`, `notes`. `Catalog::validate()` rejects duplicate
   ids, empty required fields, implausible sizes, out-of-range error rates, a measurement with no
   machine or source, and a `manifest` that is not a bare `*.json` file name.
2. Leave `download_bytes` / `disk_bytes` / `wer_estimates` **absent** unless you verified them.
   Absent renders as `—`; a fabricated number renders as a lie.
3. Add a `measurements` entry only for a run you actually performed, and fill in `machine` and
   `source` — validation requires both.
4. To make the entry installable, download the files, hash them, and commit a manifest under
   `models/manifests/`, then point the entry's `manifest` field at it.
