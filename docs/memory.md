# Background memory and model lifetime

OwlWhisp uses a Rust/iced interface rendered on the CPU with tiny-skia. It does not keep an
Electron, Chromium or WebView process alive for its interface. Local speech recognition has its
own cost: model weights, ONNX sessions, execution-provider libraries and CPU/GPU work buffers.
An unloaded client and a client keeping a speech model ready are different measurement states.

## Model lifetime

Version 0.1.1 loads the dictation model on demand. **Settings → Background memory** offers
idle timeouts of 1, 5, 15 and 30 minutes, plus **Keep model loaded**. The default is 5 minutes.
The setting is `model_idle_timeout_secs` in `settings.json`; zero disables automatic unloading.
Existing settings documents receive the default without losing their saved model or hotkey.

The timer starts after transcription finishes, including an unsuccessful attempt. Starting a new
recording cancels it; the model stays loaded throughout recording and transcription. Microphone
tests and backend-status requests do not reset the timer. Expiry releases the model and updates
the displayed backend to unloaded. The next dictation activation starts
loading the model in a background thread after the microphone starts recording. Recording and
initialization overlap; Stop waits only for the unfinished part of initialization before
transcribing. This applies both to first use and to reloading after idle expiry.

Exact GPU selections and the bundled GPU path use an isolated inference worker. Releasing it
ends that process, freeing its model, driver heaps and provider DLLs. GPU capability checks also
use short-lived probe processes. CPU/exact-provider initialization no longer loads every unrelated
GPU provider while constructing its candidate list.

These changes are available starting with v0.1.1; v0.1.0 does not include them.

## Investigation and measurements, 2026-10-03

The running desktop build occupied **5,317.79 MiB working set** and **5,178.27 MiB private commit**
in three samples over about 3.7 seconds. It used Parakeet TDT 0.6B v3 with TensorRT. Its address map
included the 2,322.6 MiB FP32 encoder weights and CUDA/TensorRT DLLs. Those mapping sizes are virtual
memory observations, not additional physical RAM to add to the working-set figure.

The old dictation loop retained its model until settings reload or application exit. Provider
probing also registered unrelated libraries into ORT's process-global environment. A fresh
TensorRT compilation reproduced several GiB retained between utterances. This establishes retained
model/runtime memory; a single observation does not establish an unbounded leak.

Validation used a **Windows x64 release build**, Parakeet TDT 0.6B v3, an Intel Core i9-13900HX and
RTX 4080 Laptop GPU with 12 GB VRAM, and the committed six-second `fleurs_en_1.wav` fixture. CPU
used the INT8 encoder; TensorRT used the FP32 ONNX graph with FP16 kernels. The lifecycle example
loaded, transcribed, released and reloaded the model twice. Both cycles produced identical text
within each backend. It opened no microphone and delivered no text to another application.

**The following table measures the inference-lifecycle example and its children, including console
helpers. It excludes the full desktop GUI.** Values are samples at each phase, not idle medians
or cross-client benchmarks. Working set and private commit measure different things.

| Lifecycle state | Working set, MiB | Private commit, MiB |
|---|---:|---:|
| After capability checks, before loading a model | 61.8 | 34.9 |
| CPU INT8, after transcription, second cycle | 802.0 | 804.7 |
| CPU INT8, after release, second cycle | 73.1 | 43.3 |
| TensorRT, after first compilation and transcription | 5,402.6 | 5,143.6 |
| TensorRT, prepared cache, after transcription, second cycle | 1,157.9 | 2,679.4 |
| TensorRT, after release, second cycle | 63.1 | 36.8 |

First TensorRT compilation peaked at approximately 7,534 MiB working set during this run. GPU VRAM
is separate and was not measured by this sampler. The lightweight-client claim concerns background
overhead with the model unloaded; it does not mean a 0.6B model uses only 60–100 MB while resident.

| Backend | First load in this run | Reload after releasing the model | Six-second fixture transcription after reload |
|---|---:|---:|---:|
| CPU INT8 | 2.44 s | 2.57 s | 1.11 s |
| TensorRT FP16 | 107.25 s, including initial compilation | 4.34 s, using the prepared cache | 0.19 s |

The 107-second compilation is not repeated on every ordinary reload: the hardware-specific
TensorRT engine cache stays on disk. Changing the model, provider, driver, hardware or cache
identity can require preparation again. The 4.34-second result assumes a compatible cache and
files already in the OS cache; a restart or a slower disk can take longer. Choose a longer idle
timeout or keep the model loaded when that delay matters more than background RAM.

## Comparing desktop clients

### Current-session snapshot, 2026-10-03

The full OwlWhisp desktop client was rebuilt with the memory changes above and restarted. The
already running OpenWhispr and Superwhisper clients were left in their existing configurations.
All three process trees were sampled together for **60.61 seconds**, with **46 complete samples
per client**, ending at 23:20:48 UTC. Hardware was the Windows x64 machine described above. No
recording or transcription workload was started during this measurement; window/tray states were
not normalized. This is an observation of these configurations, not a matched-model benchmark.

| Client/version | Processes | Private resident RAM, MiB | Total working set, MiB | Private commit, MiB |
|---|---:|---:|---:|---:|
| **OwlWhisp 0.1.0 + source memory fixes** | **1** | **43.13** | **87.91** | **49.54** |
| OpenWhispr 1.10.1 | 12 | 1,000.17 | 1,512.77 | 1,145.73 |
| Superwhisper 1.6.5 | 18 | 684.60 | 1,558.27 | 2,861.47 |

Values are medians. Private resident RAM is the sum of `WorkingSetPrivate`; total working set is
the sum of `WorkingSet`; private commit is the sum of `PrivateBytes`, from Windows process
performance counters. Private commit includes memory that is not resident in physical RAM.
Total working sets can count pages shared by multiple processes more than once. These counters
exclude GPU VRAM.

Observed model and process states:

- **OwlWhisp:** model unloaded, no inference worker. The measurement includes its native desktop
  interface, unlike the lifecycle example above. The selected model/backend remained Parakeet
  TDT 0.6B v3 / TensorRT, to be loaded on the next dictation.
- **OpenWhispr:** Parakeet TDT 0.6B v3 INT8 CPU model server running. Its command line named the
  INT8 encoder, decoder and joiner. The server alone used approximately 691 MiB private resident
  RAM. The total also includes all six Electron processes, Qdrant, the microphone listener and
  console helpers.
- **Superwhisper:** the active mode selected `cohere-transcribe-q4`, with the corresponding local
  GGUF file present. Model residency and inference backend were not verified. The process tree
  included two `Superwhisper.exe` processes and 16 `msedgewebview2.exe` descendants, identified
  by their parent chain and Superwhisper WebView profiles.

The updated OwlWhisp client had the smallest background footprint in this snapshot. Its model
was unloaded while OpenWhispr kept a model server running; these numbers do not establish a fixed
RAM advantage for the same loaded model, every configuration or another operating system.
Superwhisper's larger private commit should not be described as 2,861 MiB of physical RAM.

The [recorded samples](measurements/client-memory-2026-10-03.json) include totals, process-name
counts, build identity and model-state notes, without user paths, command lines or audio data.

### Matched-state comparison protocol

For each client, record its version, OS/architecture, hardware, speech model and quantization,
backend, model-residency setting and enabled optional features. Use the same machine, local
model/backend where supported, audio clip, window/tray state and sampling duration. Measure:

1. Idle with the model unloaded, after startup settles.
2. Idle with the model loaded, after the same transcription workload.
3. Peak during model preparation and transcription, separately from idle.
4. Idle after the configured unloading delay, if supported.

Collect three 60-second runs per idle state. Compare medians and peaks, and count the **entire
process tree**, including inference workers, Electron renderers and WebView helpers. Report
private working set, total working set and private commit separately, using the definitions above.
Whisper is a model family/reference implementation; a desktop comparison must name the specific
client that uses it.

## Repeatable Windows sampler

The read-only sampler writes JSON and leaves the application running. Supply the main process ID
from Task Manager; it discovers descendant processes on each sample and records private working
set, total working set and private commit. Use multiple IDs to sample clients in the same window.

```powershell
powershell -NoProfile -File scripts/bench/measure-client-memory.ps1 `
  -ProcessId 1234 -DurationSeconds 60 -OutputPath memory-idle.json

# In a PowerShell session, compare three already running clients simultaneously:
./scripts/bench/measure-client-memory.ps1 `
  -ProcessId @(1234, 5678, 9012) -DurationSeconds 60 -OutputPath clients-idle.json
```

To repeat inference-lifecycle QA without recording personal audio or changing saved settings:

```powershell
cargo build --release -p lw-gui --example memory-lifecycle
target/release/examples/memory-lifecycle.exe PATH_TO_SETTINGS_JSON tests/fixtures/audio/fleurs_en_1.wav
```

The example prints a JSON phase with its PID and waits for Enter before advancing. Sample that PID
from another terminal while paused at each phase. It prints timing and transcript length, not the
transcript. Settings are read only; ordinary model/runtime caches may be created during loading.
