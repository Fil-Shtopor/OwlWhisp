# LocalWisper — Licensing

_Every component that ships in a LocalWisper build, with its licence and the obligations that come
with it. Verified against source `LICENSE`/`METADATA` files and model cards on 2026-08-26._

## Application

| Component | Licence | Notes |
|---|---|---|
| LocalWisper application code | **Apache-2.0** | See `LICENSE` at the repo root. |

## Models & model assets

| Asset | Licence (SPDX) | Obligation |
|---|---|---|
| NVIDIA **Parakeet TDT 0.6B v3** weights | **CC-BY-4.0** | Attribution: name the model & NVIDIA, link to the model card, state the CC-BY-4.0 licence, and mark that the ONNX/int8/QNN forms are modifications. Shown in the About/Licences screen and `THIRD_PARTY_NOTICES`. |
| `istupakov/parakeet-tdt-0.6b-v3-onnx` (ONNX export used for CPU + NPU) | **CC-BY-4.0** (+ export author credit, MIT tooling) | Attribution as above. |
| `k2-fsa` sherpa-onnx Parakeet int8 export (alt CPU engine) | **CC-BY-4.0** (weights) / Apache-2.0 (tooling) | Attribution as above. |
| **Silero VAD** (`silero_vad.onnx`) | **MIT** | Include the MIT notice. |
| FLEURS test clips (test fixtures only, not shipped in installers) | **CC-BY-4.0** | Attribution in `tests/fixtures/audio/fixtures.json`. |

Models are **not** committed to git and **not** bundled uncompressed in the installer beyond what is
necessary; they are downloaded on first run from pinned, SHA-256-verified URLs.

## Inference runtime

| Component | Licence | Redistribution |
|---|---|---|
| **ONNX Runtime** (`onnxruntime.dll` and friends) | **MIT** (Microsoft) | Ship `ThirdPartyNotices.txt` + `LICENSE`. |
| **`onnxruntime-qnn`** EP (`onnxruntime_providers_qnn.dll`) | **MIT** (Qualcomm Technologies, Inc.) | Ship its `LICENSE`. |
| **Qualcomm QNN/QAIRT runtime** DLLs (`QnnHtp.dll`, `QnnSystem.dll`, `QnnHtpPrepare.dll`, `QnnHtpV81Stub.dll`, `libQnnHtpV81Skel.so`, `libqnnhtpv81.cat`, and the V73 set) | **Qualcomm "AI Stack License"** (proprietary; no SPDX id → declare as `LicenseRef-Qualcomm-AI-Stack-License`) | **Object code only, and only "as incorporated in Your software application"** — never as a standalone download. No reverse engineering. No removal of notices. Ship `Qualcomm_LICENSE.pdf` verbatim beside the DLLs. Excluded from the x64/non-Snapdragon packages by a forbidden-files check. |

The Qualcomm AI Stack License also lists prohibited use cases (predictive policing, social scoring,
etc.), caps liability at US$100, and is terminable by Qualcomm — these bind the *end product*, and
are surfaced in `THIRD_PARTY_NOTICES`.

## Rust dependencies (all permissive)

| Crate(s) | Licence |
|---|---|
| `tauri`, `tao`, `wry`, `tray-icon`, Tauri plugins | MIT OR Apache-2.0 |
| `ort`, `ort-sys` | MIT OR Apache-2.0 |
| `cpal`, `rubato`, `hound`, `realfft`, `rustfft`, `ndarray`, `half` | Apache-2.0 / MIT / MIT-OR-Apache-2.0 |
| `enigo` | MIT |
| `arboard` | MIT OR Apache-2.0 |
| `clipboard-win` (transitive, Windows) | BSL-1.0 |
| `global-hotkey`, `handy-keys` | MIT / Apache-2.0-OR-MIT |
| `keyring` (+ store crates) | MIT OR Apache-2.0 |
| `reqwest` (native-tls), `tokio`, `futures`, `serde`, `serde_json`, `thiserror`, `anyhow`, `tracing`, `sha2`, `hex`, `directories`, `sysinfo`, `regex`, `unicode-normalization`, `zip`, `tar`, `bzip2`, `flate2`, `clap`, `indicatif`, `uuid`, `chrono`, `parking_lot`, `crossbeam-channel`, `tokio-util` | MIT / Apache-2.0 (permissive) |
| `schannel` (Windows TLS, via native-tls) | MIT |

No copyleft (GPL/AGPL/LGPL) code is linked **in the default build**. See §"Optional sherpa engine"
below for the one build configuration where that is not automatically true. TLS uses the OS provider (SChannel/Secure Transport)
via `native-tls`, avoiding `aws-lc-rs`/`rustls` (also removes an ARM64 assembler build problem).

## Frontend dependencies

React, Vite, TypeScript, `@tauri-apps/api` and the Tauri plugin JS packages — all MIT or
Apache-2.0. Enumerated in `app/frontend/package.json`; a full SBOM is produced at release time.

## Optional `sherpa` engine — a GPL trap to avoid

`lw-engine-sherpa` (the portable CPU engine that provides Whisper, Moonshine, SenseVoice, …) is
**off by default**: without `--features sherpa` the crate `sherpa-onnx` is not in the dependency
graph at all, and the shipped binary contains none of the code below.

When the feature IS enabled, the `sherpa-onnx` crate downloads a prebuilt native archive, and the
**default archive statically links `espeak-ng`, which is GPL-3.0-or-later** (it is there for
sherpa-onnx's text-to-speech features, which LocalWisper never calls).

That matters because this app also ships the **Qualcomm QNN runtime under a proprietary licence**.
GPL-3.0 and that proprietary licence cannot both bind one binary, so a build that links espeak-ng
*and* bundles the Qualcomm DLLs must not be distributed.

**How to build the sherpa engine without GPL code:** sherpa-onnx publishes `-no-tts` archives that
omit espeak-ng (and piper-phonemize). Point the build at one instead of letting it fetch the default:

```bash
# download e.g. sherpa-onnx-v1.13.6-win-arm64-static-MT-Release-no-tts-lib.tar.bz2 into <dir>
export SHERPA_ONNX_ARCHIVE_DIR=<dir>      # or SHERPA_ONNX_LIB_DIR for pre-extracted libs
cargo build --release -p lw-cli --features sherpa
```

Before shipping any binary with `--features sherpa`, verify which archive was linked and update
`THIRD_PARTY_NOTICES.md` accordingly. Components in the no-tts archive
(sherpa-onnx Apache-2.0, ONNX Runtime MIT, kaldi-native-fbank Apache-2.0, kissfft BSD-3-Clause,
kaldi-decoder/openfst derivatives Apache-2.0) are all permissive.

Note also that `sherpa-onnx-sys`'s **build script** pulls `ureq → rustls → ring` to download that
archive. Those are build-dependencies only — they are never linked into the shipped binary — but
`ring` will not compile with MSVC on Windows ARM64, so that build additionally needs `clang` on
`PATH` (see [`build.md`](build.md)).

## Explicitly excluded (copyleft or restrictive — used only as references, never linked)

| Project | Licence | Why excluded from our binary |
|---|---|---|
| VoiceInk | GPL-3.0 | Copyleft; UX inspiration only, no code reuse. |
| Whispering (epicenter) | AGPL-3.0 | Copyleft; UX inspiration only. |
| TEN VAD | Apache-2.0 + Agora non-compete rider | Non-OSI rider; Silero (MIT) used instead. |
| Superwhisper docs | proprietary | Behavioural reference only; not redistributed. |
| `nircmd` (OpenWhispr uses it) | "free for non-commercial use" | Not OSS; not used. |

## Attribution block (shipped in About / `THIRD_PARTY_NOTICES`)

> LocalWisper uses NVIDIA Parakeet TDT 0.6B v3 (© NVIDIA, CC-BY-4.0,
> https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3), converted to ONNX by Ilya Stupakov (CC-BY-4.0)
> and to a Qualcomm QNN context binary by LocalWisper (CC-BY-4.0). Voice activity detection by Silero
> (MIT). Inference by ONNX Runtime (MIT, © Microsoft) and the Qualcomm ONNX Runtime QNN execution
> provider (MIT, © Qualcomm) with the Qualcomm AI Engine Direct runtime under the Qualcomm AI Stack
> License. Built with Tauri (MIT/Apache-2.0).
