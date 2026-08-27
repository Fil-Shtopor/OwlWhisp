# Third-Party Notices

LocalWisper (Apache-2.0) redistributes and/or builds upon the following components. Full licence
texts ship in the installer under `third-party-licenses/`. See `docs/licenses.md` for the complete
matrix and obligations.

## Models
- **NVIDIA Parakeet TDT 0.6B v3** — © NVIDIA. Licence: **CC-BY-4.0**.
  https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
  ONNX conversion by Ilya Stupakov (CC-BY-4.0), https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx
  A Qualcomm QNN context binary for Snapdragon X2 (HTP V81) is derived from these weights by
  LocalWisper and remains under CC-BY-4.0.
- **Silero VAD** — © Silero Team. Licence: **MIT**. https://github.com/snakers4/silero-vad
- Test fixtures: **FLEURS** (Conneau et al., 2022), **CC-BY-4.0**.

## Inference runtime
- **ONNX Runtime** — © Microsoft. Licence: **MIT**.
- **ONNX Runtime QNN execution provider** (`onnxruntime_providers_qnn.dll`) — © Qualcomm
  Technologies, Inc. Licence: **MIT**.
- **Qualcomm AI Engine Direct (QNN/QAIRT) runtime libraries** — © Qualcomm Technologies, Inc.
  Licence: **Qualcomm AI Stack License** (proprietary; `LicenseRef-Qualcomm-AI-Stack-License`).
  Redistributed in object-code form only, incorporated in this application, per that licence;
  `Qualcomm_LICENSE.pdf` is shipped alongside the binaries.

## Frameworks & libraries (all MIT and/or Apache-2.0 unless noted)
Tauri, tao, wry, tray-icon, and the Tauri plugins; the `ort`/`ort-sys` crates; cpal, rubato, hound,
realfft, rustfft, ndarray, half; enigo, arboard, global-hotkey, handy-keys; keyring (+ `clipboard-win`
under BSL-1.0 on Windows, via arboard); reqwest (native-TLS / `schannel` MIT), tokio, futures, serde,
serde_json, thiserror, anyhow, tracing, sha2, hex, directories, sysinfo, regex, unicode-normalization,
zip, tar, bzip2, flate2, clap, indicatif, uuid, chrono, parking_lot, crossbeam-channel, tokio-util;
React, Vite, TypeScript, @tauri-apps/api.

No GPL/AGPL/LGPL code is linked into the **default** LocalWisper build.

**Optional `sherpa` engine:** builds made with `--features sherpa` link the `sherpa-onnx` native
library. Its *default* prebuilt archive statically includes **espeak-ng (GPL-3.0-or-later)**, which
is incompatible with the proprietary Qualcomm QNN runtime this app also ships — such a build must
not be distributed. Use a `-no-tts` sherpa-onnx archive (see `docs/licenses.md`) and re-check this
file before releasing any binary with that feature enabled. Components then added are permissive:
sherpa-onnx (Apache-2.0, © k2-fsa), kaldi-native-fbank (Apache-2.0), kissfft (BSD-3-Clause).
