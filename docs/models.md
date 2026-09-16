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

The other seven entries are hash-pinned and installable, but need a build with the `sherpa` engine
feature; they are marked `requires_engine_feature: "sherpa"`:

| If you need… | Look at | Download | Why |
|---|---|---|---|
| a language Parakeet does not cover | `whisper-base` (99 languages) | 153 MiB | broad coverage, modest accuracy |
| more accuracy than `base`, same coverage | `whisper-small` | 358 MiB | ~3× the compute of `base` |
| the smallest English-only footprint | `whisper-tiny-en` | 99 MiB | the classic tiny baseline |
| short utterances / low latency, English | `moonshine-tiny-en` | 118 MiB | no fixed 30 s padding, unlike Whisper |
| the same, with more accuracy | `moonshine-base-en` | 274 MiB | ~2.3× tiny, same variable-length design |
| Chinese / Japanese / Korean / Cantonese | `sense-voice-small` | 228 MiB | one non-autoregressive pass, with punctuation |
| the top English-only quality tier | `parakeet-tdt-0.6b-v2-en` | 631 MiB | same architecture as v3, English-only; the tier is editorial, nobody here has measured it |

Every download figure above is the exact sum of the pinned file sizes in that entry's manifest, not
a rounded upstream claim. All seven pin the **int8** exports only: for `whisper-small` that is the
difference between a 358 MiB and a ~925 MiB install, for a quality difference nobody here has
measured. If you want the fp32 graphs, they sit in the same upstream repository — add a second
`artifacts` set with target `any` and the hashes you compute yourself.

Rules of thumb the catalog encodes: **quality tier** is an editorial ranking of the model family,
not a measurement; **speed tier** is the input to the RTF estimate; and neither tells you how the
model does on *your* audio, accent, or vocabulary. The only way to know that is §5.

> **`lw` cannot run these yet.** `lw-cli` depends on `lw-engine-parakeet` and has no dependency on
> `lw-engine-sherpa`, and no `sherpa` cargo feature of its own — only `app/src-tauri` wires
> `sherpa = ["lw-engine-sherpa/sherpa"]`. So `lw models install` will fetch and verify any of the
> seven entries above, but `lw transcribe --model-dir` cannot then transcribe with one. Until the
> feature is wired through the CLI, the way to run an installed sherpa model is the engine's own
> integration test:
>
> ```sh
> LW_SHERPA_MODEL_DIR=<models>/moonshine-tiny-en \
>   cargo test --release -p lw-engine-sherpa --features sherpa \
>   --test transcribe_fixture -- --ignored --nocapture
> ```

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
error: `<id>` has no pinned manifest, so there is nothing safe to download.
Its file set has not been hash-pinned yet (see `lw models info <id>`); LocalWisper refuses
to fetch unverified model files. A manifest is added once the files are downloaded, hashed and
committed under models/manifests/.
```

Every entry in the shipped catalog is pinned today, so nothing hits that path — but it is what
guards the next entry somebody adds.

### Why the sherpa models are pinned from Hugging Face, not from the GitHub release

sherpa-onnx publishes its models on the `asr-models` release of `k2-fsa/sherpa-onnx`, where every
asset is a **`.tar.bz2` archive**. `ModelDownloader` fetches files; it does not unpack archives (see
"what pinning an archive would need" below). It also publishes no SHA-256 for any asset.

The same files are published **loose** in the sherpa-onnx author's own Hugging Face repositories
(`huggingface.co/csukuangfj/sherpa-onnx-*`), which is what the manifests pin — each URL carries a
fixed commit revision, so the bytes behind it cannot change under us. Hugging Face publishes no
checksum we trust either, so **every SHA-256 in `models/manifests/` was computed locally from the
downloaded bytes**, never copied from upstream metadata.

**What it would take to pin an archive instead.** `FileEntry` is `{path, url, bytes, sha256}` and
`ModelDownloader::install` streams each URL to `staging_dir/path`, hashes it, and promotes the
directory. Nothing in that path can expand anything. Supporting archives would need, at minimum:
an explicit `archive` field on `FileEntry` naming the format (the schema must not sniff it from the
URL), extraction into staging after the hash check and never before, a path-traversal guard on every
entry inside the archive at least as strict as `FileEntry::path_is_safe`, a decompressed-size bound
so a zip bomb cannot fill the disk, a second expected size for the free-space pre-check (which today
sums `bytes`, the *compressed* total), and a rule for the single top-level directory these archives
all carry. That is a real feature with a real attack surface; pinning loose files avoided needing it.

### Where models live, and keeping the app and the CLI in agreement

The **desktop app** keeps models inside its own Tauri app-data directory, next to `settings.json`:

| Platform | App models directory |
|---|---|
| Windows | `%APPDATA%\ai.localwisper.app\models` |
| macOS | `~/Library/Application Support/ai.localwisper.app/models` |
| Linux | `~/.local/share/ai.localwisper.app/models` |

The **CLI** defaults somewhere else (`%LOCALAPPDATA%\LocalWisper\models` /
`~/.local/share/LocalWisper/models`), because it is usable without the app ever being installed.
That means the two can disagree about what is installed unless you tell them not to. Set
`LW_MODELS_ROOT` (or pass `--dest`) to point the CLI at the app's copy:

```bash
LW_MODELS_ROOT="$APPDATA/ai.localwisper.app/models" lw models list
#   parakeet-tdt-0.6b-v3 ... INSTALLED: yes
```

Both read install state through the same `lw_core::model::paths` code, so with the same root they
always give the same answer.

### From the app

Settings → **Models** shows the same catalog, marks the entry recommended for your hardware, and
downloads through the same verified path with a progress bar and a cancel button. Cancelling is not
an error: partial files stay in staging and the next attempt resumes them.

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
   `models/manifests/`, then point the entry's `manifest` field at it. Compute the hashes from the
   bytes you downloaded; never copy a checksum out of upstream metadata. Pin URLs that cannot move
   under you — a Hugging Face `/resolve/<commit sha>/` path, not `/resolve/main/`.
5. Two tests in `lw-core` hold you to it: `builtin_manifest_references_resolve_to_a_valid_pinned_file`
   checks that every `manifest` names a file that exists, parses, validates and carries the same
   `id`; `builtin_pinned_sizes_match_the_manifest_they_name` checks that `download_bytes` is the
   real sum of the pinned file sizes, so the number in the catalog cannot drift from the manifest.
