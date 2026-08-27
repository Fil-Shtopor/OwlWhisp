# lw-engine-sherpa

A **portable CPU** speech engine for LocalWisper, implementing `lw_core::engine::SpeechEngine` on
top of the official [`sherpa-onnx`](https://crates.io/crates/sherpa-onnx) Rust bindings (k2-fsa,
Apache-2.0, pinned at 1.13.6).

Where `lw-engine-parakeet` runs one model through our own ONNX Runtime layer — and reaches the
Qualcomm NPU by doing so — this crate trades acceleration for **choice and reach**: it runs
Whisper, Moonshine, SenseVoice, Paraformer and NeMo/Zipformer transducers on any x86-64 or aarch64
CPU (Snapdragon, Intel, AMD, Apple), with the decoders supplied by sherpa-onnx rather than written
here.

```
model directory ──▶ detect::detect_in_dir ──▶ ModelFiles ──▶ sherpa OfflineRecognizer
                    (file names only)          (+ SherpaConfig)      │
                                                                     ▼
                                           AudioBuffer (16 kHz mono f32) ──▶ Transcript
```

## The `sherpa` cargo feature

The native dependency downloads a ~140 MB prebuilt archive during `build.rs`, so it is **opt-in**:

```toml
lw-engine-sherpa = { workspace = true, features = ["sherpa"] }
```

`cargo check --workspace` therefore stays fast for people who do not want that download. Without
the feature the crate still compiles and still exports `detect`, `lang` and `SherpaConfig` —
everything that reasons about models without loading them — but `SherpaEngine` is absent. Code that
must build either way should branch on `lw_engine_sherpa::is_available()`.

## Two ONNX Runtimes in one process — and why this works

`lw-ort` loads Microsoft's `onnxruntime.dll` **dynamically, by full path** (`ort`'s `load-dynamic`).
sherpa-onnx carries its own ONNX Runtime. The workspace pins sherpa's `static` feature (its
default), so that second runtime is **linked into the binary** rather than dropped next to ours as a
colliding `onnxruntime.dll`:

```toml
sherpa-onnx = { version = "1.13.6", default-features = false, features = ["static"] }
```

The two never see each other's symbols. Windows resolves our copy through an explicit
`LoadLibrary` handle (no ELF-style global interposition), and sherpa's copy is bound at link time.

This is **verified**, not assumed: `tests/dual_runtime.rs` builds one binary containing both, then
initializes `lw_ort::OrtRuntime`, runs a sherpa transcription, builds a real Microsoft-ORT CPU
session over one of the same `.onnx` graphs, transcribes again, and (when the QNN provider is
present) registers the Qualcomm plugin EP and transcribes once more. Measured on Windows 11 ARM64
(Snapdragon X, MSVC 14.51, rustc 1.98): all steps pass and sherpa's output is byte-identical before
and after.

## Expected model layouts

Point `SherpaConfig::model_dir` at an extracted upstream archive from the
[sherpa-onnx `asr-models` release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models).
The family is auto-detected from the file names alone (`src/detect.rs`, fully unit-tested — no model
download needed to test it):

| Kind | Required files |
|---|---|
| `Whisper` | `<name>-encoder.onnx`, `<name>-decoder.onnx`, `<name>-tokens.txt` (e.g. `tiny.en-encoder.onnx`) |
| `Moonshine` v1 | `preprocess.onnx`, `encode.onnx`, `uncached_decode.onnx`, `cached_decode.onnx`, `tokens.txt` |
| `Moonshine` v2 | `encoder.onnx`, `merged_decoder.onnx`, `tokens.txt` |
| `NemoTransducer` | `encoder.onnx`, `decoder.onnx`, `joiner.onnx`, `tokens.txt` (Parakeet TDT, Zipformer transducer) |
| `SenseVoice` | `model.onnx`, `tokens.txt`, directory name mentioning `sense-voice` |
| `Paraformer` | `model.onnx`, `tokens.txt`, directory name mentioning `paraformer` |

Any `.onnx` may also appear as `<stem>.int8.onnx` (also `.fp16`, `.q8`, `.int4`, `.quant`);
`SherpaConfig::prefer_quantized` (default **true**) chooses between them and falls back to whichever
variant exists. A bare `model.onnx` is genuinely ambiguous between SenseVoice and Paraformer, so the
directory name breaks the tie — or set `SherpaConfig::model_kind` explicitly. If `model_dir` holds
no recognizable layout but has exactly one subdirectory, that subdirectory is tried too (extracting
a `.tar.bz2` leaves the model one level down).

## Usage

```rust
use lw_core::engine::SpeechEngine;
use lw_engine_sherpa::{SherpaConfig, SherpaEngine};

let mut engine = SherpaEngine::open(
    SherpaConfig::new(model_dir)
        .with_threads(4)          // 0 = half the machine, clamped to [1, 8]
        .with_language("ru"),     // multilingual Whisper / SenseVoice only
)?;

let transcript = engine.transcribe(&audio)?;   // audio: 16 kHz mono f32, resampled if needed
println!("{} ({} on {})", transcript.text, engine.backend_name(), engine.device().name);
```

`SherpaEngine::new(config)` + `initialize(&ctx)` is the registry-friendly form; `initialize` is
idempotent (it will not reload a multi-hundred-megabyte graph because it was called twice) and takes
`model_dir` / `cpu_threads` from the `EngineInitContext` when they are set.

The engine reports itself honestly: `provider() == Provider::NativeCpu`,
`acceleration() == Acceleration::Cpu`, `device().name == "CPU (N threads)"`,
`backend_name()` is the detected family (`whisper`, `moonshine`, `nemo-transducer`, `sense-voice`,
`paraformer`), `supports_streaming() == false`. `health_check()` decodes 0.5 s of silence and
reports the real latency.

## Measured results

Real runs on this machine — Windows 11 ARM64, Snapdragon X (12 cores), 8 intra-op threads, `dev`
profile (`opt-level = 1`, deps at 2; the sherpa native library is a prebuilt release build).
Fixture `tests/fixtures/audio/fleurs_en_1.wav` (6.00 s) and `fleurs_ru_1.wav` (5.64 s); WER against
`tests/fixtures/audio/fixtures.json` with the same normalization `lw-engine-parakeet`'s fixture test
uses. RTF is warm (second pass); the cold pass was within ~10 % in every case.

| Model directory | Detected kind | English WER | RTF | Token timings |
|---|---|---|---|---|
| `sherpa-onnx-whisper-tiny.en` (int8) | `whisper` | 0.150 | 0.025 | none |
| `sherpa-onnx-whisper-tiny.en` (fp32) | `whisper` | 0.150 | 0.028 | none |
| `sherpa-onnx-moonshine-tiny-en-int8` | `moonshine` | 0.200 | 0.013 | none |
| `sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8` | `nemo-transducer` | 0.150 | 0.024 | 35 tokens |
| `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17` | `sense-voice` | 0.100 | 0.010 | 29 tokens |

Russian, `fleurs_ru_1.wav`, Parakeet TDT v3 int8: **WER 0.000**, RTF 0.022 —
`Основной религией в Молдавии является православное христианство.`

Whisper tiny.en, English: `There are many beaches due to Auckland straddling of two harbors and most
popular ones are in three areas.`

`health_check()` on Whisper tiny.en: `ok`, 44 ms for the 0.5 s silent probe.

## Wiring this into the CLI or the app

This crate deliberately does not touch `crates/lw-cli` or `app/`. Everything a caller needs is
public, and nothing below requires the `sherpa` feature *except* `SherpaEngine` itself.

Add the dependency where the engine is selected:

```toml
# crates/lw-cli/Cargo.toml (or app/src-tauri/Cargo.toml)
lw-engine-sherpa = { workspace = true, features = ["sherpa"] }
```

Then, for a `--engine sherpa --model-dir <dir>` style flag:

```rust
use lw_core::engine::{EngineInitContext, SpeechEngine};
use lw_engine_sherpa::{SherpaConfig, SherpaEngine};

let mut cfg = SherpaConfig::new(&model_dir).with_threads(threads);
if let Some(lang) = language { cfg = cfg.with_language(lang); }

let mut engine: Box<dyn SpeechEngine> = Box::new(SherpaEngine::new(cfg));
engine.initialize(&EngineInitContext {
    model_dir,
    cache_dir,          // unused by this engine; pass whatever the registry has
    cpu_threads: threads,
})?;
```

`SherpaEngine::open(cfg)` does both steps and returns the concrete type, which is handier for a
one-shot CLI command. Note the engine needs **no** `OrtRuntime` and no `runtime/` directory — that
is the point of it.

Useful before loading a model (all available without the `sherpa` feature, so `lw models`-style
listing code stays cheap):

| Call | Use |
|---|---|
| `lw_engine_sherpa::is_available()` | `true` when built with the `sherpa` feature — branch on this instead of `cfg!` |
| `detect_in_dir(&dir, prefer_quantized, hint)?` | Identify a downloaded model directory; returns `ModelFiles` |
| `files.kind()` / `ModelKind::ALL` / `kind.as_str()` | Names for `--engine` / `--model-kind` values and help text |
| `"whisper".parse::<ModelKind>()?` | Parse a `--model-kind` flag (accepts `parakeet`, `sense_voice`, … ) |
| `lang::languages_for(&files)` | Languages a specific model supports |
| `lang::languages_for_kind(kind)` | Languages a family supports, before download |
| `cfg.language_for(&files)?` | Validate `--language` against the model *without* loading it |
| `cfg.validate()?` | Reject a bad `--decoding-method` / thread count early |
| `files.verify_exists()?` | Report a half-downloaded model directory precisely |

For diagnostics, `engine.device()` returns `DeviceInfo { name: "CPU (8 threads)", detail:
Some("sherpa-onnx whisper (greedy_search), bundled ONNX Runtime CPU provider") }`, and
`engine.model_kind()` / `engine.model_files()` expose what was actually loaded.

## Known limits

- **No streaming.** sherpa-onnx does have streaming recognizers, but for a different set of models
  (streaming Zipformer/Paraformer). Every family loaded here is offline.
- **Whisper token timestamps are usually unavailable.** `whisper_token_timestamps` is off by
  default because the stock `sherpa-onnx-whisper-*` archives are exported without cross-attention
  outputs; turning it on makes sherpa log a warning and return no timestamps. Transducer and
  Moonshine models emit timestamps unconditionally, and those are mapped into `Transcript::tokens`.
- **CPU only.** `provider` is pinned to `"cpu"`. sherpa can target CUDA/CoreML, but that is not what
  this crate is for.
- **Coverage is uneven, on purpose.** Whisper, Moonshine v1, NeMo transducer and SenseVoice have
  each been run against a real downloaded model (see the table above). **Paraformer** and
  **Moonshine v2** are implemented and their detection is unit-tested, but no real model of either
  has been loaded here — treat them as untested until someone runs one.
- **Whisper language auto-detection is sherpa's**, not ours: with no `language` hint a multilingual
  export detects the language internally, and `Transcript::language` stays `None` because the C API
  does not report what it picked. Pass a hint if the caller needs to know.

## Building on Windows ARM64

`sherpa-onnx-sys`'s build script depends on `ureq` → `rustls` → **`ring`**, and `ring` 0.17 hard-
requires `clang` on `aarch64-pc-windows-msvc` (it refuses to compile its assembly with MSVC). So a
`clang` for this target must be on `PATH` in addition to the usual MSVC environment:

```sh
source ~/.msvc-arm64/env-arm64.sh
export PATH="$HOME/.cargo/bin:$HOME/lwdev/llvm/bin:$PATH"   # clang.exe lives here
cargo build --features sherpa
```

A portable extraction of `clang+llvm-<ver>-aarch64-pc-windows-msvc` (just `bin/clang.exe` plus
`lib/clang/*/include`, ~400 MB) is enough; no system-wide LLVM install is required. On x86-64
Windows, Linux and macOS the MSVC/GCC/Clang toolchain already present is sufficient.

The native archive is cached under `target/sherpa-onnx-prebuilt/`. Set `SHERPA_ONNX_ARCHIVE_DIR`
(directory holding a pre-downloaded `.tar.bz2`) or `SHERPA_ONNX_LIB_DIR` (directory of prebuilt
`.lib`/`.a` files) to build without network access.

## Tests

```sh
# Everything that needs no model — detection, config validation, language tables, result mapping.
cargo test -p lw-engine-sherpa                      # 37 tests, no native dependency
cargo test -p lw-engine-sherpa --features sherpa    # + the engine's own unit tests

# Model-backed integration tests (all #[ignore]d, paths from the environment).
export LW_SHERPA_MODEL_DIR=<models>/sherpa-onnx-whisper-tiny.en
export LW_SHERPA_MULTILINGUAL_MODEL_DIR=<models>/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8
export LW_RUNTIME_DIR=runtime/win-arm64
cargo test -p lw-engine-sherpa --features sherpa -- --ignored --nocapture
```

Optional knobs for the fixture tests: `LW_SHERPA_THREADS`, `LW_SHERPA_FP32=1` (skip the `.int8`
variants), `LW_SHERPA_WHISPER_TIMESTAMPS=1`.

## Licensing

`sherpa-onnx` and `sherpa-onnx-sys` are Apache-2.0. The prebuilt archive bundles ONNX Runtime (MIT),
kaldi-native-fbank, kaldi-decoder, OpenFST, espeak-ng (GPL-3.0 — used only by the TTS paths, which
this crate never calls) and piper-phonemize. Record these in `THIRD_PARTY_NOTICES.md` before
shipping a binary that enables the `sherpa` feature.
