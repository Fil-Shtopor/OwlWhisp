# LocalWisper — Hardware support

_What runs where today, what is verified on real hardware, and exactly what each missing path would
take. Updated 2026-08-27._

The rule this project follows: **a backend is only claimed once it has been observed executing.**
Everything below is labelled accordingly.

| Legend | Meaning |
|---|---|
| ✅ **verified** | Executed and measured on that hardware |
| ⚙️ **implemented, untested** | Code path exists and compiles; nobody has run it on that chip yet |
| 🧩 **needs work** | Not implemented; the section says what it would take |

---

## 1. Support matrix

| Hardware | CPU inference | Accelerator | Status |
|---|---|---|---|
| **Snapdragon X2 Elite** (SC8480XP, Hexagon **V81**) | ✅ RTF 0.032 | ✅ **NPU** via QNN/HTP, RTF 0.0145 | **verified** — see [`benchmarks.md`](benchmarks.md) |
| **Snapdragon X Elite / X Plus** (SC8380XP, Hexagon **V73**) | ⚙️ same code path | ⚙️ NPU via QNN/HTP | **implemented, untested** — see §2 |
| Any **x86-64 CPU** (Intel / AMD), Windows | ⚙️ ORT CPU EP | — | **implemented, untested** — see §3 |
| **Intel Core Ultra** NPU (Meteor/Lunar/Arrow Lake) | ⚙️ CPU works | 🧩 NPU | needs the OpenVINO EP — see §4 |
| **AMD Ryzen AI** NPU (XDNA) | ⚙️ CPU works | 🧩 NPU | needs the Vitis AI EP — see §4 |
| **Apple Silicon** (M-series) | ⚙️ ORT CPU EP | 🧩 ANE / GPU | needs a CoreML encoder backend — see §5 |
| Intel Mac | ⚙️ ORT CPU EP | — | ORT dropped macOS x86-64 after 1.24 |
| **Linux** x86-64 / ARM64 | ⚙️ ORT CPU EP | 🧩 CUDA / others | platform layer is scaffolding — see §6 |

Nothing here is architecture-locked: the engine picks an [`EncoderBackend`](architecture.md) at
runtime and reports the real one in Diagnostics, so adding a chip means adding a backend, not
touching the UI, the audio pipeline or the text pipeline.

---

## 2. Snapdragon X Elite / X Plus (Hexagon V73)

**This is the closest to working.** The Hexagon generation differs per SoC — X Elite/X Plus are
**V73**, X2 Elite is **V81** — and a QNN context binary is valid only for the generation it was
prepared for. Everything needed is already in place:

- `lw-platform`'s capability detector reads the generation from the installed NPU driver package
  (`libQnnHtpV<NN>Skel*.so` in the DriverStore) and maps it to the QNN `soc_model`
  (V73 → 60, V81 → 88). Both mappings are unit-tested.
- The engine now takes that value from detection instead of assuming one SoC
  (`ParakeetConfig::with_capabilities`), so an X Elite selects `htp_arch=73`.
- The context-binary cache key includes the architecture, so a V73 and a V81 context can never be
  confused for one another.
- The runtime we ship includes the **V73** stub/skel/cat files alongside V81.

**What is unverified:** nobody has run it on an X Elite. On first use the app would prepare a V73
context on-device (a few minutes, once) exactly as it does for V81. If it fails, the engine falls
back to CPU and says so. Testing on an X Elite is the single highest-value next step for hardware
coverage.

---

## 3. Ordinary Intel / AMD CPUs

The CPU path has no Qualcomm dependency at all: it is ONNX Runtime's CPU execution provider running
the same Parakeet ONNX model. To build for x86-64 Windows:

```powershell
pwsh -File scripts\runtime\fetch-runtime.ps1     # stage the runtime (see note below)
cargo build --release -p lw-cli --target x86_64-pc-windows-msvc
```

Two caveats:

1. The staging script currently fetches the **ARM64** ONNX Runtime and the Qualcomm QNN DLLs. For an
   x64 build you want `onnxruntime-win-x64-<version>.zip` and **no** QNN DLLs — the release manifest
   already treats the Qualcomm binaries as forbidden in non-Snapdragon packages (a licence
   requirement, see [`licenses.md`](licenses.md)). The runtime loader looks in `runtime/win-x64/`.
2. Expected speed: Parakeet's encoder is ~600 M parameters, so an int8 CPU run is roughly
   "a few × faster than real time" on a modern desktop core — usable for dictation, far slower than
   the NPU. Measure it with `lw bench` rather than trusting an estimate.

**Status: implemented, untested** — no x64 machine was available here.

---

## 4. Intel and AMD NPUs

Both are reachable through ONNX Runtime execution providers we do not currently ship:

| Vendor | EP | What it needs |
|---|---|---|
| Intel (Core Ultra NPU) | **OpenVINO EP** | Ship `onnxruntime_providers_openvino.dll` + the OpenVINO runtime; register it the same way `lw-ort` registers QNN; a static-shape, quantized encoder export |
| AMD (Ryzen AI / XDNA) | **Vitis AI EP** | Ryzen AI software stack + a quantized model compiled for XDNA |

The integration point is small and already abstracted: `lw-ort` registers a plugin execution
provider and enumerates its devices; `lw-engine-parakeet` picks an `EncoderBackend`. Adding Intel
would mean an `OpenVinoEncoder` alongside `QnnHtpEncoder`, plus a per-vendor model artifact.

The hard part is not the code — it is the **model artifact**. Each NPU wants its own quantized,
static-shape encoder, and each needs to be produced and validated on that hardware. Promising
support without a machine to verify it on would violate this project's first principle.

There is also **Windows ML** (Windows App SDK), which distributes vendor EPs through an OS-managed
catalog on Copilot+ PCs. That could eventually give Intel/AMD/Qualcomm NPUs behind one API. Today its
published requirements list only Snapdragon X Elite/X Plus, and it lags Qualcomm's own QNN releases,
so we bundle the QNN EP ourselves instead. Worth revisiting.

**Status: not implemented.**

---

## 5. Apple Silicon

macOS builds compile: audio (cpal/CoreAudio), clipboard, settings, the model manager and the ORT CPU
path are all cross-platform. What is missing is acceleration:

- **ORT CoreML EP** — the cheapest option. Needs a static-shape fp16 encoder export and
  `MLComputeUnits` configured; the `ort` crate already exposes the CoreML EP options. Reported to be
  unstable with dynamic-shape Parakeet graphs, which is why static shapes matter.
- **A CoreML bridge to FluidAudio's `.mlmodelc` bundles** — published measurements put the Parakeet
  encoder at ~28 ms per 15 s window with 99 % of ops on the ANE. That is a separate, verified-by-
  others artifact we could load through `objc2-core-ml`.

Neither is implemented, and — important — **no Apple hardware was available**, so any ANE claim would
be unverifiable. The macOS platform module also still needs its hotkey (CGEventTap), text injection
(Cmd+V via CGEventPost with the pasteboard transaction) and non-activating NSPanel overlay.

**Status: CPU path compiles; acceleration not implemented, nothing verified.**

---

## 6. Linux

The workspace builds and the ORT CPU path works. `lw-platform`'s Linux module is scaffolding:
audio via cpal compiles, while global hotkeys, text injection and the overlay return
`Unavailable`. Wayland in particular needs the `GlobalShortcuts` portal for hotkeys and the
`RemoteDesktop` portal (or uinput) for injection.

**Status: scaffolding.**

---

## 7. Choosing a model for your hardware

Model choice and hardware interact: the NPU path needs a static-shape encoder prepared for that
Hexagon generation, while CPU models run anywhere. The catalog records, per model, which hardware
targets it supports, and `lw models list` marks the ones recommended for the machine it is run on.
See [`models.md`](models.md) — and treat the pre-download numbers there as **estimates**, with
`lw bench` as the way to get real ones on your own machine.

---

## 8. How to add a new accelerator

1. Implement `EncoderBackend` (see `crates/lw-engine-parakeet/src/encoder.rs`) — one `run()` taking
   mel features and returning encoder output.
2. Register the EP in `lw-ort` (mirror `register_qnn`), keeping registration **lazy** so it is never
   auto-applied to CPU sessions.
3. Extend `lw_core::capabilities` + `lw-platform`'s detector so the device is discovered honestly.
4. Add the model artifact to the catalog with its hardware target.
5. Add it to `SpeechEngine::health_check` reporting so Diagnostics shows the truth.

Steps 1–3 are each on the order of a hundred lines. Step 4 — producing and validating the quantized
artifact on real silicon — is the work that actually gates a new chip.
