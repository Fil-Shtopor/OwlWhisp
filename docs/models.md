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

**On a Snapdragon X / X2 laptop: `parakeet-tdt-0.6b-v3`.** It is the only entry the default build
can run, the only one with a Qualcomm NPU path, and the only one measured on an accelerator
(RTF 0.032 CPU / 0.0145 NPU, word-weighted WER 5.4 % / 4.8 % —
[§5.1](benchmarks.md#51-measured-lw-bench-results-on-the-x2-2026-08-26-native-arm64-lwexe)). It
covers 25 European languages with punctuation and casing. Cost: a 640 MiB download.

The other sixteen entries are hash-pinned and installable, but need a build with the `sherpa`
engine feature (see the box at the end of this section); they are marked
`requires_engine_feature: "sherpa"`.

### 3.1 What has actually been measured here

Eleven of the seventeen entries carry a `measurements` point taken on this project's target machine
(Snapdragon X2 Elite Extreme X2E94100, CPU only, 8 threads) with `lw bench` over
`tests/fixtures/audio` — 12 FLEURS clips, three each in **en, es, ru, uk**. Nothing in this table
came from an upstream claim.

| Model | Download | Measured WER | Mean RTF | Scored over |
|---|---|---|---|---|
| `whisper-turbo` | 989 MiB | **0.042** | 0.469 | 12 clips, en/es/ru/uk |
| `parakeet-tdt-0.6b-v3` | 640 MiB | **0.054** | 0.032 | 12 clips, en/es/ru/uk |
| `whisper-medium` | 902 MiB | **0.054** | 1.250 | 12 clips, en/es/ru/uk |
| `whisper-distil-small-en` | 285 MiB | **0.062** | 0.075 | 3 clips, en |
| `parakeet-tdt-ctc-110m-en` | 455 MiB | **0.062** | 0.014 | 3 clips, en |
| `paraformer-en` | 219 MiB | **0.077** | 0.011 | 3 clips, en |
| `moonshine-tiny-en` | 118 MiB | **0.092** | 0.0125 | 3 clips, en |
| `paraformer-trilingual-zh-yue-en` | 233 MiB | **0.169** | 0.011 | 3 clips, en (of zh/yue/en) |
| `paraformer-zh` | 217 MiB | **0.277** | 0.011 | 3 clips, en (a Chinese model) |

> **Read the "scored over" column before you compare two rows.** A 3-clip English figure and a
> 12-clip four-language figure are not the same measurement. Twelve clips are indicative, three are
> barely that: a zero means "no word errors on 20 seconds of
> clean read speech after normalisation", not "these models do not make mistakes", and the fixtures
> cannot tell them apart. Each entry's `measurements[].source` spells this out; `lw models info <id>`
> prints it.

Two rows need their footnote read twice. `paraformer-zh` is a **Chinese** model and the fixture set
has no Chinese at all, so the only clips it could be scored on are the English three — 0.277 is a
real number about its secondary language, and this catalog contains **no** measurement of its
actual one. `paraformer-trilingual-zh-yue-en` has the same problem for two of its three languages.
The same gap applies to `sense-voice-small` (zh/ja/ko/yue), which is why it carries no measurement
at all.

`lw bench` scores WER only on clips in languages the engine claims, so an English-only model is not
punished for the Russian clips. That protection is family-wide, though: `lw-engine-sherpa` reports
the whole 25-language NeMo list for *any* offline transducer, so a monolingual transducer export
would be scored against languages it cannot speak. Run against the full 12 clips, a Russian-only
transducer
scores 0.784; against the three Russian ones, 0.000. The three monolingual transducer entries
(`parakeet-tdt-ctc-110m-en`) were therefore measured on a
language-subset copy of the fixture directory, and each says so in its `source`.

### 3.2 Picking one

| If you need… | Look at | Download | Why |
|---|---|---|---|
| the best accuracy measured here, any of 100 languages | `whisper-turbo` | 989 MiB | large-v3's encoder, 4-layer decoder; WER 0.042, RTF 0.469 |
| the classic large Whisper | `whisper-medium` | 902 MiB | WER 0.054 — but RTF 1.250, i.e. slower than real time |
| a language Parakeet does not cover, cheaply | `whisper-base` (99 languages) | 153 MiB | broad coverage, modest accuracy |
| more accuracy than `base`, same coverage | `whisper-small` | 358 MiB | ~3× the compute of `base` |
| fast English, distilled | `whisper-distil-small-en` | 285 MiB | 2 decoder layers instead of 12; ~10× quicker than `whisper-small` |
| English with punctuation and casing, small | `parakeet-tdt-ctc-110m-en` | 455 MiB | NVIDIA's 110M Parakeet; full precision, no int8 published |
| the smallest English-only footprint | `whisper-tiny-en` | 99 MiB | the classic tiny baseline |
| short utterances / low latency, English | `moonshine-tiny-en` | 118 MiB | no fixed 30 s padding, unlike Whisper |
| the same, with more accuracy | `moonshine-base-en` | 274 MiB | ~2.3× tiny, same variable-length design |
| Chinese / Japanese / Korean / Cantonese | `sense-voice-small` | 228 MiB | one non-autoregressive pass, with punctuation |
| Mandarin specifically | `paraformer-zh` | 217 MiB | Alibaba's Paraformer-large; non-autoregressive, RTF 0.011 |
| Mandarin + Cantonese + English | `paraformer-trilingual-zh-yue-en` | 233 MiB | SeACo-Paraformer; the only other Cantonese option here |
| English from the same family | `paraformer-en` | 219 MiB | WER 0.077 at RTF 0.011; no punctuation or casing |
| the top English-only quality tier | `parakeet-tdt-0.6b-v2-en` | 631 MiB | same architecture as v3, English-only; the tier is editorial, nobody here has measured it |

Every download figure above is the exact sum of the pinned file sizes in that entry's manifest, not
a rounded upstream claim. All but one pin the **int8** exports only: for `whisper-small` that is the
difference between a 358 MiB and a ~925 MiB install, for a quality difference nobody here has
measured. If you want the fp32 graphs, they sit in the same upstream repository — add a second
`artifacts` set with target `any` and the hashes you compute yourself. The exception is
`parakeet-tdt-ctc-110m-en`, which is pinned at full precision because the upstream `-int8`
repository publishes no loose files at all: its quantized weights exist only inside the `.tar.bz2`
on the GitHub release, which the installer cannot unpack.

Rules of thumb the catalog encodes: **quality tier** is an editorial ranking of the model family,
not a measurement; **speed tier** is the input to the RTF estimate; and neither tells you how the
model does on *your* audio, accent, or vocabulary. The only way to know that is §5.

> **These need a `sherpa` build of the CLI.** The default `lw` links only `lw-engine-parakeet`, so
> `lw models install` will fetch and verify any of the sixteen sherpa entries but `lw transcribe`
> and `lw bench` will not load one. Build the CLI with the feature — the extra step keeps GPL-3.0
> espeak-ng out of the binary, see [build.md](build.md) and [licenses.md](licenses.md):
>
> ```sh
> $env:SHERPA_ONNX_LIB_DIR = (pwsh -File scripts/build/fetch-sherpa.ps1 -Quiet)
> cargo build --release -p lw-cli --features sherpa
> ```
>
> `build_engine` then picks the engine from what is in the directory, so
> `lw bench tests/fixtures/audio --model-dir <models>/whisper-turbo` just works. The engine's own
> integration test is still there if you want a single-clip smoke check:
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
   `source` — validation requires both. You are already downloading the files to hash them, so
   **run the model before you delete them**: `lw bench tests/fixtures/audio --model-dir <dir>`
   costs one command and turns an editorial tier into a number. Copy the line it prints —
   `word-weighted WER: 0.042   over 12 clip(s) in en/es/ru/uk` — into `source` verbatim. That
   phrase is the whole point of the field: without it, nobody can tell a 3-clip English figure
   from a 12-clip multilingual one, and the two will be compared as though they measured the same
   thing.
4. **One measurement per (model, hardware).** If you take a better one — more clips, a rebuilt
   bench — replace the old point rather than adding to it, and say in `source` that it replaces
   something. Two figures for the same model on the same machine that disagree by a factor of two
   are worse than either alone.
5. Check what `lw bench` decided to score. It asks the engine for `supported_languages()` and
   skips clips in languages the model does not claim — but `lw-engine-sherpa` answers with a
   *family* list, so any offline transducer claims all 25 European languages of the multilingual
   NeMo releases. A monolingual transducer export therefore gets scored against languages it
   cannot speak (a Russian-only transducer measured 0.784 over all 12 clips and 0.000 over the
   Russian three). When the
   printed language list is wider than the model, run against a language-subset copy of
   `tests/fixtures/audio` and say so in `source`. Conversely, when the fixtures cannot reach a
   model's real language at all — Chinese, Cantonese, Japanese — do not let the number stand
   unqualified; `paraformer-zh`'s notes are the pattern.
6. To make the entry installable, download the files, hash them, and commit a manifest under
   `models/manifests/`, then point the entry's `manifest` field at it. Compute the hashes from the
   bytes you downloaded; never copy a checksum out of upstream metadata. Pin URLs that cannot move
   under you — a Hugging Face `/resolve/<commit sha>/` path, not `/resolve/main/`.
7. Get the licence from the model's own LICENSE file or model card, never from the family it
   belongs to. The two Alibaba entries here disagree: `sense-voice-small` carries the custom,
   non-SPDX `LicenseRef-FunASR-Model-1.1`, while the three Paraformer entries are Apache-2.0 per
   their ModelScope pages. A repository can also ship a file called `LICENSE` that is not one —
   the GigaAM export's is a saved GitHub web page.
8. Mind the `local_dir`. `lw-engine-sherpa` cannot tell SenseVoice from Paraformer by file layout
   (both are `model.onnx` + `tokens.txt`) and breaks the tie on the directory name, so a Paraformer
   manifest's `local_dir` must keep the word `paraformer` in it.
9. Two tests in `lw-core` hold you to it: `builtin_manifest_references_resolve_to_a_valid_pinned_file`
   checks that every `manifest` names a file that exists, parses, validates and carries the same
   `id`; `builtin_pinned_sizes_match_the_manifest_they_name` checks that `download_bytes` is the
   real sum of the pinned file sizes, so the number in the catalog cannot drift from the manifest.
