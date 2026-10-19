# Android and iOS architecture assessment

Assessed on 2026-10-19 against commit `f43b291` (workspace version 0.1.9).
This is a source and platform-documentation review. Android/iOS compilation,
device inference, and keyboard integration have not been validated by this assessment.
The structure below is proposed, not implemented.

## Decision

Keep desktop, Android, and iOS in this repository while establishing a shared Rust SDK.
Give the native applications separate build pipelines and release versions. Revisit repository
separation after both mobile targets can consume the same small, explicit SDK interface.

Adding iOS strengthens the case for a common SDK and two native application adapters. It does
not itself establish a need for another Git repository. Moving code to another repository would
leave its current desktop dependencies intact and add dependency-version coordination during
the period in which the SDK boundary will change most often.

## Existing code to reuse

| Existing component | Reuse in Android/iOS | Required boundary or qualification |
|---|---|---|
| `lw-core/src/audio` | PCM buffers, format conversion, resampling | Native applications own microphone capture and audio-session lifecycle; pass audio chunks into Rust. |
| `lw-core/src/engine` | `SpeechEngine`, transcript/token types, language and actual-provider reporting | Expose a small facade instead of exporting internal Rust traits directly. |
| `lw-core/src/vad` | Endpoint detection and its configuration | Native hosts supply lifecycle events; the specific VAD implementation remains replaceable. |
| `lw-core/src/text`, `dictionary` | Normalization, replacements, deterministic text processing | Make the optional HTTP LLM processor separable from the local text pipeline. |
| `lw-core/src/model` and `models/` | Manifests, languages, attribution, hashes, file validation and installation logic | Supply sandbox paths explicitly; keep native background-download scheduling outside the SDK. Filter models by actual mobile engine/artifact support. |
| `lw-engine-parakeet` | Mel frontend, TDT decoder, vocabulary and transcript merging | Adapt runtime initialization and linking; desktop accelerator support does not prove mobile support. |
| `lw-engine-sherpa` | Model-layout detection and offline engine adapter | Upstream supports Android/iOS, but this project's pinned Rust/native linkage needs a mobile build proof. |
| `lw-vad-silero` | VAD implementation | Depends on the same ORT packaging work. |
| `lw-core/src/bench`, audio fixtures | Comparable correctness and performance checks | Record mobile hardware, cold/warm latency and peak memory; desktop results do not predict phone results. |
| Selected `lw-app` logic | Pipeline construction, engine selection, model views and measurement data | Extract useful decisions from desktop paths, settings, hardware probes and process orchestration. |

## Confirmed boundaries that need work

1. **`lw-core` is a useful foundation, but not a ready mobile SDK.** Its manifest includes
   `reqwest`, TLS, archives, filesystem helpers and hardware-related dependencies. Its settings
   combine model/VAD/dictionary preferences with desktop hotkeys, overlays, autostart and tray
   startup. Separate reusable configuration from desktop configuration without breaking existing
   settings files. Use optional features or focused extraction where mobile builds justify it.
2. **Platform policy has no Android/iOS variants.**
   `lw-core/src/capabilities/platform.rs` recognizes Windows, macOS and Linux; Android/iOS become
   `Unknown`. The accelerator policy currently treats CoreML as macOS-only. Extend the platform
   model deliberately, including device versus simulator where native artifacts differ.
3. **`lw-app` remains desktop orchestration.** Its dictation worker owns `lw-platform::Capture`
   and delivers text through foreground-window inspection, text injection and clipboard fallback.
   `provider_worker.rs` launches the current executable as a subprocess. Mobile applications must
   own capture, text delivery and suspension/interruption behavior through their native APIs.
4. **ORT initialization assumes desktop distribution.** `lw-ort` enables `load-dynamic` and
   desktop provider features. Runtime discovery walks executable ancestors and runtime folders.
   Android/iOS need explicit packaged-library initialization and target-specific dependencies.
   In particular, choose a supported iOS linking/package route and verify the pinned ORT API
   compatibility before attempting engine reuse. Prefer one inference runtime in the initial
   mobile binary rather than automatically carrying over both desktop ORT copies.
5. **Downloads have two different purposes.** Preserve model-data verification and resumable
   transfers. The desktop runtime/provider downloader and application updater do not belong in
   the iOS application. Ship executable inference libraries with the application; handle app
   distribution through the platform. Review native download scheduling and backup/cache paths.
6. **Streaming is not currently implemented by either main engine adapter.** Both
   `lw-engine-parakeet` and `lw-engine-sherpa` return `false` from `supports_streaming()`.
   Live partial text needs a separate implementation and measurement. A streaming interface or
   microphone UI alone is not evidence of incremental inference.
7. **Current CI and releases cover desktop.** CI checks the entire workspace on desktop targets,
   and the release workflow triggers on `v*` tags. Add separate Android/iOS jobs that build
   selected SDK packages, with the Apple builds on a Mac runner. Keep existing desktop tags
   compatible; define mobile tag namespaces and release versions explicitly.

## Proposed dependency structure

```text
existing desktop app        Android app                  iOS app
lw-app / lw-gui / CLI       Kotlin / Compose / IME        Swift / SwiftUI
          |                         |                         |
          |                   generated Kotlin          generated Swift
          |                         +------------+------------+
          |                                      |
          |                                    lw-ffi
          +----------------------+---------------+
                                 |
                               lw-sdk
                                 |
                   lw-core + selected engine + VAD
```

Keep existing desktop crate locations initially. A possible addition is:

```text
crates/lw-sdk/       # Toolkit-independent session and engine orchestration
crates/lw-ffi/       # Narrow Kotlin/Swift API generated from the same interface
mobile/android/     # Native application and Android input-method services
mobile/ios/         # Native application; extensions only after a separate UX proof
models/             # Shared metadata with target-aware artifact availability
```

UniFFI is a candidate for generating Kotlin and Swift bindings. It does not build or package
the Rust/native libraries for the mobile platforms; NDK/Xcode integration remains separate work.
An initial SDK should accept explicit model/cache paths, configuration and buffered PCM audio;
load/transcribe/release a model; apply text processing; and report progress, errors and actual
execution providers. Keep microphone permissions, OS text insertion, secrets, UI, app updates,
and background scheduling in the native hosts.

Design cancellation and stale-result rejection at the session boundary. Add explicit resource
release for memory pressure and app suspension; individual engines may need additional work
before they can interrupt an inference call. Do not promise streaming through this API until an
adapter provides it. Run inference off the native UI thread and pass audio in chunks rather than
making one language-boundary call per sample.

## iOS product feasibility

Local recognition is feasible: ONNX Runtime and sherpa-onnx both document iOS deployment.
Keyboard-based dictation needs its own feasibility gate:

- Apple's extension guide states that custom keyboards have no direct microphone access.
- Current keyboard documentation describes a separate extension process with a device-dependent
  memory limit. Keep heavyweight inference in the main application for the initial design.
- The current App Review Guidelines, section 4.4.1, require usable keyboard input and keyboard
  switching, functionality without full access, and prohibit launching other apps except
  Settings. Do not base the plan on a keyboard button simply opening the containing app.
- A containing-app recording session plus a keyboard extension requires a dedicated prototype
  and review of public APIs, background behavior and distribution requirements. Its smoothness
  and acceptance are not established by the existence of an iOS inference library.
- A first iOS prototype can record/transcribe in the main application and expose the result via
  copy/share. Evaluate a keyboard extension or Shortcuts integration as separate user flows.
  Replacing the built-in Apple keyboard's microphone provider is not an established integration.

These platform constraints apply regardless of repository placement.

## When a separate repository becomes useful

| Arrangement | Assessment |
|---|---|
| One repository: shared SDK + desktop + native Android/iOS | Recommended during SDK extraction and the first device prototypes. One change can update engine, bindings and consumers together. |
| Current repository provides a versioned SDK; `OwlWhisp-Mobile` contains Android and iOS | Reasonable after the SDK boundary stabilizes, or if mobile gets a separate team/access policy. Requires explicit SDK revisions, compatible native artifacts and consumer testing. |
| Separate desktop, Android, iOS and SDK repositories immediately | Adds release/dependency coordination while all four boundaries are still changing. No demonstrated benefit for the current project. |

If mobile is later moved out, consume a pinned SDK version and its matching native artifacts.
Keep the model manifest source of truth explicit. Avoid copied Rust modules or an implicit
dependency on another repository's current `main` branch.

## Work order and acceptance evidence

1. Define the narrow SDK inputs/outputs and separate desktop configuration/path assumptions.
2. Build one selected offline engine for Android ARM64 and iOS ARM64 using the same fixture and
   model revision. Exercise generated Kotlin/Swift bindings in a real host. Use a Mac for iOS.
3. Verify device transcription, model loading/unloading, cancellation behavior, audio interruption,
   memory-pressure handling, offline execution and peak memory. Establish CPU results before
   promising CoreML, GPU or Qualcomm acceleration.
4. Prove Android IME insertion/return on the target Samsung. Separately prove the desired iOS
   capture-to-text workflow; ordinary in-app transcription is an earlier milestone.
5. Add platform CI and independent app releases. Reconsider splitting repositories against the
   actual SDK churn and ownership requirements observed during these prototypes.

The inspected Windows machine has Android SDK/NDK directories, Java and adb. Only the Windows
x64 Rust target is currently installed. No Android or iOS compilation was attempted in this
assessment; the presence of SDK directories does not establish a working mobile build.

## Platform references

- [UniFFI: shared Rust logic and Kotlin/Swift bindings](https://mozilla.github.io/uniffi-rs/latest/)
- [ONNX Runtime Mobile](https://onnxruntime.ai/docs/get-started/with-mobile.html)
- [ONNX Runtime iOS build prerequisites](https://onnxruntime.ai/docs/build/ios.html)
- [sherpa-onnx Android](https://k2-fsa.github.io/sherpa/onnx/android/index.html)
- [sherpa-onnx iOS](https://k2-fsa.github.io/sherpa/onnx/ios/index.html)
- [Apple: creating a custom keyboard](https://developer.apple.com/documentation/uikit/creating-a-custom-keyboard)
- [Apple extension guide: keyboard capabilities and microphone restriction](https://developer.apple.com/library/archive/documentation/General/Conceptual/ExtensibilityPG/CustomKeyboard.html)
- [App Review Guidelines: 2.5.2, 2.5.4, 4.4.1](https://developer.apple.com/app-store/review/guidelines/)
