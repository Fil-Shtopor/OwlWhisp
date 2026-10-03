# OwlWhisp — Choosing a model

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
   gigaam-v3-ru          GigaAM v3 Russian, with punctu… sherpa         221.2 MiB     1 very-fast  best       ~0.026 yes
   whisper-turbo         Whisper large-v3-turbo (sherpa… whisper        988.6 MiB   100 moderate   best       ~0.188 no
   parakeet-tdt-ctc-110… NVIDIA Parakeet TDT-CTC 110M e… nemo_transdu…  455.3 MiB     1 very-fast  better     ~0.026 no
   sense-voice-small     SenseVoice Small (int8, sherpa… sense_voice    228.5 MiB     5 very-fast  better     ~0.026 no
   moonshine-tiny-en     Moonshine tiny en (int8, sherp… moonshine      118.2 MiB     1 very-fast  good       ~0.026 yes
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

The other six entries are hash-pinned and installable, and the shipped application runs them
(it links the `sherpa` engine; a default `cargo build` of the CLI does not — see the box at the end
of this section). They are marked
`requires_engine_feature: "sherpa"`.

### 3.1 What has actually been measured here

Every one of the eight entries carries a `measurements` point taken on this project's target machine
(Snapdragon X2 Elite Extreme X2E94100, CPU only, 8 threads) with `lw bench` over
`tests/fixtures/audio` — 15 FLEURS clips, three each in **en, es, ru, uk, zh**. Nothing in this table
came from an upstream claim.

Every figure below comes from **one sweep on 2026-09-18**: each model in turn, on the CPU, over the
same fixture set. The CPU rather than the NPU because the CPU is the only accelerator all of them can
use — sherpa-onnx links its own static ONNX Runtime with the CPU provider, so
`parakeet-tdt-0.6b-v3` is the only entry here with any other path at all.

**A model is run only on the languages it claims.** Clips in other languages are skipped rather than
transcribed and then left out of the score. Both halves matter: transcribing them wasted time that
landed in the RTF with nothing to mark it as unreal, so an English-only model used to carry a speed
averaged over nine clips of Spanish, Russian and Ukrainian. The Chinese clips are therefore absent
from this table for every entry — they are scored as a character rate, which cannot be averaged with
a word rate, and they appear in [the Chinese table](#chinese) instead.

Two exports — `gigaam-v3-ru` and `parakeet-tdt-ctc-110m-en` — carry no language metadata at all, so
the engine claims nothing and every clip would count. The app falls back to the catalog's language
list for those; `lw bench` needs an explicit `--languages`.

| Model | Role | Download | Measured WER | Warm RTF | Scored over |
|---|---|---|---|---|---|
| `gigaam-v3-ru` | fast, accurate | 221 MiB | **0.029** | 0.0149 | 3 clips, ru |
| `whisper-turbo` | accurate, universal | 989 MiB | **0.042** | 0.1844 | 12 clips, en/es/ru/uk |
| `parakeet-tdt-0.6b-v3` | fast | 640 MiB | **0.054** | 0.0401 | 12 clips, en/es/ru/uk |
| `parakeet-tdt-ctc-110m-en` | fast | 455 MiB | **0.062** | 0.0099 | 3 clips, en |
| `sense-voice-small` | — | 228 MiB | **0.062** | 0.0125 | 3 clips, en (of zh/ja/ko/yue/en) |
| `qwen3-asr-0.6b` | — | 838 MiB | **0.071** | 0.0901 | 9 clips, en/es/ru |
| `moonshine-tiny-en` | compact | 118 MiB | **0.092** | 0.0102 | 3 clips, en |
| `omnilingual-300m` | — | 279 MiB | **0.144** | 0.0297 | 12 clips, en/es/ru/uk |

**Warm RTF, not the all-clip mean.** This is the mean over every clip *after* the first — the same
quantity the app records and shows under "on your machine", so the two can be read side by side.
`lw bench` prints it next to the cold RTF, which is the first clip alone and carries one-time session
warm-up; on this sweep the cold clip ran 10–50% slower than the warm figure. An earlier revision of
this table recorded the all-clip mean instead, which is a different quantity, and that alone made the
catalog look like it disagreed with the app.

**Read the WER column, and treat the RTF column as a ranking.** Each RTF is the median of three
back-to-back runs. The WER was identical in all three for every model — decoding is greedy and
deterministic — while the RTF was not.

**Machine state dominates the RTF, so it is recorded.** This sweep was taken with the machine idle
(CPU load 2–6%, the app closed) and the run-to-run spread was 2–8%. The revision before it was taken
while the machine was busy with an IDE, a browser, a sync client and compiler jobs: its spread
reached 59%, and every sherpa entry came out 20–40% slower. Those figures were withdrawn rather than
adjusted. Two entries make the point on their own — `gigaam-v3-ru` and `parakeet-tdt-ctc-110m-en`
were re-measured by exactly the same command with no methodology change and still moved by a quarter
and a third. An RTF here says this model is faster than that one on this hardware; it does not say
what the silicon can do.

`whisper-turbo`'s RTF was 0.469 in an earlier, separate run. The accuracy reproduced exactly; the
speed did not, and the old figure was taken outside this sweep under conditions that were not
written down. The number above is the one measured under the conditions stated.

#### The same multilingual models, per language

A single blended figure hides the thing that decides which one you want. On identical clips:

These per-language rates are **stored in the catalog as data**, under each measurement's
`per_language`, not only written up here. That is what lets the app's "Which one should you use?"
block answer a per-language question with per-language evidence. It used to rank every role by one
global order led by the editorial quality tier, so choosing Chinese suggested `whisper-turbo` for
accuracy — the best entry by blended WER and the worst of the four on Chinese, 0.380 against
SenseVoice's 0.141. Each role now ranks by its own criterion (fewest errors, least delay, smallest
download, most languages), and for accuracy it uses the rate measured on the chosen language.

Where a language has no fixtures — Cantonese, Japanese, Korean — the pick says so rather than
quietly ranking on a figure from other languages.

| Model | en | es | ru | uk |
|---|---|---|---|---|
| `whisper-turbo` | 0.031 | **0.000** | **0.029** | **0.148** |
| `qwen3-asr-0.6b` | **0.031** | 0.024 | 0.206 | *not claimed* (0.852) |
| `parakeet-tdt-0.6b-v3` | 0.062 | 0.024 | 0.059 | 0.074 |
| `omnilingual-300m` | 0.185 | 0.024 | 0.088 | 0.296 |
| `gigaam-v3-ru` | — | — | **0.029** | — |

Qwen3-ASR ties Whisper turbo on English at well under half the real-time factor, and is the worst Russian
here of anything that claims Russian — 3.5× Parakeet and 7× GigaAM. That is why it carries no
role: `accurate` reads as a general claim, and on the languages these fixtures can score,
`whisper-turbo` matches or beats it everywhere. Its 0.852 on the Ukrainian clips is *not* in its
total, because its model card does not claim Ukrainian; that exclusion is the whole point of
scoring only claimed languages.

#### Chinese

Both blockers named in the previous version of this section are gone.

1. **Chinese audio** — three FLEURS `cmn_Hans` clips were added on 2026-09-18 by
   `scripts/benchmarks/add_fixtures_language.py`, which appends rather than regenerating: the
   twelve existing clips are untouched, because every measurement in this catalog was taken over
   them.
2. **A metric that works** — `lw bench` now scores languages written without word delimiters by
   **character** (CER) rather than by word. See
   [benchmarks.md](benchmarks.md#words-or-characters) for why a word rate on Chinese has only two
   possible values.

Measured with `--languages zh` over the three `cmn_Hans` clips, each figure the median of three
back-to-back runs on 2026-09-18. These are **character** error rates and are kept out of the blended
totals above, because a word rate and a character rate are not the same quantity:

| Model | CER | Warm RTF | Download |
|---|---|---|---|
| `sense-voice-small` | **0.141** | **0.0109** | **228 MiB** |
| `qwen3-asr-0.6b` | 0.155 | 0.0924 | 838 MiB |
| `omnilingual-300m` | 0.310 | 0.0292 | 279 MiB |
| `whisper-turbo` | 0.380 | 0.2126 | 989 MiB |

So the best Chinese in this catalog is also the smallest and the fastest of the four that claim it.
That settles a question this document had been carrying: `sense-voice-small` is not made redundant by
the newer entries. The CER was identical in all three runs; only the RTFs moved.

**`whisper-turbo` holds the `universal` role and is the worst of the four here** — 2.7× SenseVoice's
error at 20× the real-time factor. The role is earned on en/es/ru/uk and does not carry over: someone
who picks it for its hundred languages and then dictates Chinese gets the weakest option in the
catalog. This is exactly the kind of thing a single blended accuracy figure would have hidden.

Three clips is indicative and not statistically meaningful, and one of them (`fleurs_zh_2`) embeds a
Latin word in parentheses that every model here stumbles on. The ranking is worth more than any one
figure. Japanese, Korean and Cantonese remain unmeasured — there are no fixtures for them.

> **Read the "scored over" column before you compare two rows.** A 3-clip English figure and a
> 12-clip four-language figure are not the same measurement. Twelve clips are indicative, three are
> barely that: a zero means "no word errors on 20 seconds of
> clean read speech after normalisation", not "these models do not make mistakes", and the fixtures
> cannot tell them apart. Each entry's `measurements[].source` spells this out; `lw models info <id>`
> prints it.

Two rows need their footnote read twice, and this section changed its mind about them, which is
worth recording rather than smoothing over.

On 2026-09-17 two Paraformer entries were removed partly *because* their only measurements were
English figures for Chinese models — 0.277 and 0.169 — and this document argued that no number was
better than a number about the wrong language. On 2026-09-18 `sense-voice-small` and
`qwen3-asr-0.6b` were measured on English anyway, and both now carry a figure.

The reversal is deliberate, and rests on one distinction. Those Paraformer rows presented their
English as *the* accuracy of a Chinese model, with nothing next to the number saying otherwise. The
two rows here name the languages they were scored on in the table itself, and their `notes` say in
capitals that this catalog holds no measurement of the languages they exist for. A measured figure
that is labelled as what it is beats a blank, because a blank is where a reader puts a guess — and
0.062 on English is a real and useful fact about `sense-voice-small`, as long as nobody reads it as
its Chinese.

What has not changed is the rule that produced both decisions: never let a number stand where it
could be read as describing something it does not describe.

`lw bench` scores WER only on clips in languages the engine claims, so an English-only model is not
punished for the Russian clips.

That protection used to have a hole worth recording. `lw-engine-sherpa` reported the whole
25-language NeMo list for *any* offline transducer, because the `encoder/decoder/joiner` layout
carries no language metadata and the family spans everything from a multilingual NeMo release to a
monolingual export. A Russian-only transducer therefore claimed 25 languages and scored **0.784**
over the full 12 clips where it scores **0.000** over the three Russian ones — a number that was
about to be printed as its accuracy.

A detected transducer now claims nothing, which is the truth its files support; the catalog states
languages per entry, which is where a fact the files do not carry belongs. `lw bench` prints a
**per-language breakdown** whenever the clips span more than one language, and when a model claims
nothing it refuses to print a single blended figure at all, naming the languages instead.
`--languages ru` states what a model handles when its files cannot. The measurement for
`parakeet-tdt-ctc-110m-en` predates that change and was taken on a language-subset copy of the
fixture directory; its `source` says so.

### 3.2 Picking one

The catalog carries **seven** entries, each with a job no other entry does better. Five of the seven
declare a `roles` list — `fast`, `accurate`, `universal`, `compact` — which is what the app's
"pick by what you need" strip resolves against. Roles are editorial, like the quality and speed
tiers; the measurement beside them is the evidence.

| If you need… | Look at | Role | Download | Why |
|---|---|---|---|---|
| Russian, and nothing else | `gigaam-v3-ru` | fast, accurate | 221 MiB | WER **0.029** against Parakeet v3's 0.059 on the same three Russian clips, at warm RTF 0.0149, and it emits punctuation |
| 25 European languages, quickest | `parakeet-tdt-0.6b-v3` | fast | 640 MiB | the built-in default and the only entry with a Qualcomm NPU path; WER 0.054 across en/es/ru/uk |
| the best accuracy measured here, or a language nothing else covers | `whisper-turbo` | accurate, universal | 989 MiB | large-v3's encoder with a 4-layer decoder; WER 0.042 over 100 languages, but RTF 0.469 |
| English, fastest at that accuracy | `parakeet-tdt-ctc-110m-en` | fast | 455 MiB | WER 0.062 at warm RTF 0.0099; punctuation and casing |
| the smallest thing worth using | `moonshine-tiny-en` | compact | 118 MiB | WER 0.092 at warm RTF 0.0102, and no fixed 30 s padding, so cost scales with what you actually said |
| Chinese, Japanese, Korean or Cantonese, compactly | `sense-voice-small` | — | 228 MiB | one non-autoregressive pass with punctuation, and the best Chinese here at CER 0.141 / warm RTF 0.0109; its English figure of 0.062 is a *secondary* language |
| the widest language coverage, including Chinese dialects | `qwen3-asr-0.6b` | — | 838 MiB | 30 languages and 22 Chinese dialects; the joint best English here, and poor Russian — read the per-language table above |

There is deliberately **no `live` role**. Live transcription means partial text appearing while you
speak, and this application has no such path: the hotkey opens a capture, releasing it closes one,
and the utterance is transcribed whole. Tagging a model `live` would advertise a mode the product
does not have, whatever the model can do elsewhere.

#### What was removed, and why

The catalog was cut from fifteen entries to six on 2026-09-17. Everything dropped is in git
history, so restoring any of it is a revert. The cuts, with the evidence:

| Removed | Because |
|---|---|
| `whisper-medium` | `whisper-turbo` beats it on **both** axes measured here — 0.042 at RTF 0.469 against 0.054 at RTF 1.250 — in a smaller download. Keeping both offered a strictly worse choice. |
| `whisper-small`, `whisper-base`, `whisper-tiny-en` | never measured here; all three are the weaker end of a family whose measured end is `whisper-turbo`. A cheap multilingual model that mis-transcribes is a false economy when the text is typed straight into another application. |
| `whisper-distil-small-en` | same WER as `parakeet-tdt-ctc-110m-en` (0.062) at five times the real-time factor (0.075 against 0.014). |
| `parakeet-tdt-0.6b-v2-en` | 631 MiB, English-only, never measured, and superseded by the v3 entry that is the default. |
| `moonshine-base-en` | never measured; `moonshine-tiny-en` holds the compact role with a real number. |
| `paraformer-en` | 219 MiB for WER 0.077, against `moonshine-tiny-en`'s 118 MiB for 0.092 and `parakeet-tdt-ctc-110m-en`'s 0.062. It won neither axis. |
| `paraformer-zh`, `paraformer-trilingual-zh-yue-en` | their only measurement here was on English, a **secondary** language for one and absent from the other's name — the fixtures contain no Chinese or Cantonese at all. `sense-voice-small` covers the same languages and carries no misleading number. |

Every download figure above is the exact sum of the pinned file sizes in that entry's manifest, not
a rounded upstream claim. Most pin the **int8** exports only, for a quality difference nobody here
has measured; if you want the fp32 graphs, they sit in the same upstream repositories — add a
second `artifacts` set with target `any` and the hashes you compute yourself. `gigaam-v3-ru` is
int8 in its encoder only, which is how it is published. The exception is
`parakeet-tdt-ctc-110m-en`, which is pinned at full precision because the upstream `-int8`
repository publishes no loose files at all: its quantized weights exist only inside a `.tar.bz2`
on the GitHub release. That is no longer a hard limit — see below — but the entry has not been
re-pinned yet.

#### Archives

The installer can now fetch a `.tar.bz2` and unpack it, which is what unlocked `qwen3-asr-0.6b`:
sherpa-onnx publishes 498 models on one GitHub release and mirrors almost none of them as loose
files. A manifest entry carrying an `extract` block pins the **archive's** SHA-256, and the
archive is opened only after that hash matches — never before. Extraction confines every entry to
the model directory, refuses links, refuses any path with `..`, a root or a drive prefix, and caps
the entry count and the unpacked size. `only_prefixes` keeps the test audio and shell scripts that
ship beside the model out of the model directory. The rules, and why each one is there, are in
`crates/lw-core/src/model/archive.rs`; two of its tests build deliberately hostile archives by
hand, because `tar::Builder` will not write one.

Rules of thumb the catalog encodes: **quality tier** is an editorial ranking of the model family,
not a measurement; **speed tier** is the input to the RTF estimate; and neither tells you how the
model does on *your* audio, accent, or vocabulary. The only way to know that is §5.

> **The installed application runs these. A default-built `lw` does not.** The shipping build
> script passes `--features sherpa` to both binaries, but a plain `cargo build -p lw-cli` links
> only `lw-engine-parakeet`, so `lw models install` will fetch and verify any of the six
> sherpa entries while `lw transcribe` and `lw bench` refuse to load one. Build the CLI with the
> feature — the extra step is what keeps GPL-3.0 espeak-ng out of the binary, see
> [build.md](build.md) and [licenses.md](licenses.md):
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
lw models install parakeet-tdt-0.6b-v3 --dest "$LOCALAPPDATA/OwlWhisp/models"
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
Its file set has not been hash-pinned yet (see `lw models info <id>`); OwlWhisp refuses
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

The **desktop app** keeps models inside its own app-data directory, next to `settings.json`:

| Platform | App models directory |
|---|---|
| Windows | `%APPDATA%\ai.owlwhisp.app\models` |
| macOS | `~/Library/Application Support/ai.owlwhisp.app/models` |
| Linux | `~/.local/share/ai.owlwhisp.app/models` |

The **CLI** defaults somewhere else (`%LOCALAPPDATA%\OwlWhisp\models` /
`~/.local/share/OwlWhisp/models`), because it is usable without the app ever being installed.
That means the two can disagree about what is installed unless you tell them not to. Set
`LW_MODELS_ROOT` (or pass `--dest`) to point the CLI at the app's copy:

```bash
LW_MODELS_ROOT="$APPDATA/ai.owlwhisp.app/models" lw models list
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
   unqualified, and prefer no measurement to a misleading one: `sense-voice-small` ships with an
   empty `measurements` for exactly this reason.
6. To make the entry installable, download the files, hash them, and commit a manifest under
   `models/manifests/`, then point the entry's `manifest` field at it. Compute the hashes from the
   bytes you downloaded; never copy a checksum out of upstream metadata. Pin URLs that cannot move
   under you — a Hugging Face `/resolve/<commit sha>/` path, not `/resolve/main/`.
7. Get the licence from the model's own LICENSE file or model card, never from the family it
   belongs to. The four Alibaba entries here disagree with each other:
   `sense-voice-small` carries the custom, non-SPDX `LicenseRef-FunASR-Model-1.1`, while the three
   Paraformer entries are Apache-2.0 per their ModelScope pages. A repository can also ship a file
   called `LICENSE` that is not one — in an export reviewed for this catalog it was a saved copy of
   a GitHub web page, which grants nothing.
8. Mind the `local_dir`. `lw-engine-sherpa` cannot tell SenseVoice from Paraformer by file layout
   (both are `model.onnx` + `tokens.txt`) and breaks the tie on the directory name, so a Paraformer
   manifest's `local_dir` must keep the word `paraformer` in it.
9. Two tests in `lw-core` hold you to it: `builtin_manifest_references_resolve_to_a_valid_pinned_file`
   checks that every `manifest` names a file that exists, parses, validates and carries the same
   `id`; `builtin_pinned_sizes_match_the_manifest_they_name` checks that `download_bytes` is the
   real sum of the pinned file sizes, so the number in the catalog cannot drift from the manifest.
