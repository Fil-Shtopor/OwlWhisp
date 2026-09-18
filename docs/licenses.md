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

### Models the user may choose to download

The catalog also offers models LocalWisper does **not** redistribute: the user downloads them from
the publisher, and the licence binds them, not us. Each entry states its licence before anything is
fetched, and `lw models info <id>` prints it.

| Model | Licence | Note |
|---|---|---|
| Whisper large-v3-turbo (sherpa-onnx export, int8) | **MIT** | the openai/whisper repository is MIT; the `openai/whisper-*` Hugging Face cards say Apache-2.0. Read both before redistributing. |
| Moonshine tiny en (int8) | **MIT** | — |
| NVIDIA Parakeet TDT-CTC 110M en | **CC-BY-4.0** | attribution as for v3 |
| **Qwen3-ASR 0.6B** (int8) | **Apache-2.0** | From the `Qwen/Qwen3-ASR-*` model cards. Shipped by sherpa-onnx only as a `.tar.bz2` on a GitHub release, so this is the first entry whose manifest pins an **archive** hash rather than per-file hashes — the archive is verified before it is opened, and nothing inside it is trusted to name its own destination. |
| **GigaAM v3 Russian** (punct, int8 encoder) | **MIT** | Copyright (c) 2024 GigaChat Team. The Hugging Face `license` metadata field on the export is **empty**; the licence comes from the 1070-byte `LICENSE` file in the repository, which was read in full. That check is not ceremonial — an earlier GigaAM v2 export shipped a file named `LICENSE` that was a saved copy of a GitHub web page and granted nothing. |
| **SenseVoice Small** (int8) | **`LicenseRef-FunASR-Model-1.1`** — *not* SPDX, not OSI-approved | Alibaba's FunASR Model Open Source License Agreement v1.1. Permits use, copying, modification and sharing; requires attribution and retention of model names; disclaims all liability; and **terminates if you "denigrate" the software**. Materially more restrictive than everything else here. Read it before redistributing anything built with it. |

SenseVoice is listed rather than hidden because a user is entitled to choose it with the terms in
front of them — but it is the one entry whose licence would need a deliberate decision before this
project shipped anything derived from it.

Models are **not** committed to git and **not** bundled uncompressed in the installer beyond what is
necessary; they are downloaded on first run from pinned, SHA-256-verified URLs.

## Inference runtime

| Component | Licence | Redistribution |
|---|---|---|
| **ONNX Runtime** (`onnxruntime.dll` and friends) | **MIT** (Microsoft) | Ship `ThirdPartyNotices.txt` + `LICENSE`. |
| **`onnxruntime-qnn`** EP (`onnxruntime_providers_qnn.dll`) | **MIT** (Qualcomm Technologies, Inc.) | Ship its `LICENSE`. |
| **WebGPU plugin EP** (`onnxruntime_providers_webgpu.dll`, `dxcompiler.dll`, `dxil.dll`) | **MIT** (Microsoft) — bundles Dawn (BSD-3-Clause) and DirectXShaderCompiler (LLVM/NCSA + MIT) | Ship its `LICENSE` and `ThirdPartyNotices`. Portable GPU support on win-x64, win-arm64, osx-arm64 and linux-x64; no vendor SDK and no proprietary component. |
| Vendor EPs **not** shipped: CUDA, TensorRT, DirectML, OpenVINO, Vitis AI | MIT (the EP) over vendor redistributables with their own terms | Not bundled. Each needs a vendor SDK on the machine, and none could be verified here — see [`hardware.md`](hardware.md) §5. A user who installs one gets it detected automatically. |
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

No copyleft (GPL/AGPL/LGPL) code is linked in any shipped build. The one configuration where that
could go wrong — the optional `sherpa` engine — is covered below, along with the build step that
prevents it. TLS uses the OS provider (SChannel/Secure Transport)
via `native-tls`, avoiding `aws-lc-rs`/`rustls` (also removes an ARM64 assembler build problem).

## Frontend dependencies

React, Vite, TypeScript, `@tauri-apps/api` and the Tauri plugin JS packages — all MIT or
Apache-2.0. Enumerated in `app/frontend/package.json`; a full SBOM is produced at release time.

## The `sherpa` engine — a GPL trap, and how this project avoids it

`lw-engine-sherpa` provides Whisper, Moonshine, SenseVoice and NeMo/Zipformer models. **The
shipped Windows build links it**, so the statement below is about the binary users actually get,
not about an optional extra. Enabling it is a build-time decision (`--features sherpa`, passed by
`scripts/build/build-windows-arm64.ps1` after it stages the right libraries), and it comes with a
licensing hazard worth stating precisely, because the obvious build is the wrong one.

**The hazard.** The `sherpa-onnx-sys` crate downloads a prebuilt native archive. The archive it
chooses by default statically links **espeak-ng, GPL-3.0-or-later**, together with
piper_phonemize and ucd. Those exist for sherpa-onnx's *text-to-speech* features, which LocalWisper
never calls. But this application also ships the **Qualcomm QNN runtime under a proprietary
licence**, and GPL-3.0 and that licence cannot both bind one work. A build that links espeak-ng
*and* bundles the Qualcomm libraries must not be distributed.

**The fix, and the wrinkle.** sherpa-onnx publishes `-no-tts` archives that omit all three. Every
remaining component is permissive:

| Component | Licence |
|---|---|
| sherpa-onnx | Apache-2.0 |
| ONNX Runtime (sherpa's own static copy) | MIT |
| kaldi-native-fbank, kaldi-decoder, kaldifst, openfst derivatives | Apache-2.0 |
| kissfft | BSD-3-Clause |
| ssentencepiece | Apache-2.0 |

The wrinkle: `sherpa-onnx-sys` emits `-l static=espeak-ng`, `-l piper_phonemize` and `-l ucd`
**unconditionally**, so linking against a no-tts archive fails with *"could not find native static
library"*. There is no crate feature to turn that off, in any published version.

`scripts/build/fetch-sherpa.ps1` resolves it: it fetches the no-tts archive and generates three
**empty** static libraries under those names. Nothing references their symbols — the no-tts build
of sherpa-onnx-core was compiled without TTS — so the link succeeds and no TTS code of any licence
enters the binary. The stubs satisfy a spurious flag; they stand in for nothing. The script also
refuses to run if a future archive starts shipping a real espeak-ng again.

```powershell
$env:SHERPA_ONNX_LIB_DIR = (pwsh -File scripts\build\fetch-sherpa.ps1 -Quiet)
cargo build --release -p lw-cli --features sherpa
```

**Verified** on Windows ARM64, at two levels. The staged library directory contains no espeak-ng,
piper_phonemize or ucd beyond the 1 KB stubs, the build links, and all 44 `lw-engine-sherpa` tests
pass. And because a clean input directory is an argument rather than a guarantee, the linked
`localwisper.exe` was itself searched: 390 occurrences of `sherpa-onnx`, 24 of `OfflineRecognizer`,
36 of `kaldi` — and **zero** of `espeak` or `piper_phonemize`. The engine is in the shipped binary;
the GPL-3.0 component is not.

`sherpa-onnx-sys`'s **build script** additionally pulls `ureq → rustls → ring` to download the
archive. Those are build-dependencies only and are never linked into the shipped binary.

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
