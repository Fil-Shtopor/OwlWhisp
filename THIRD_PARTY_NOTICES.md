# Third-Party Notices

OwlWhisp (Apache-2.0) redistributes and/or builds upon the following components. Full licence
texts for the bundled NVIDIA runtime ship beside its DLLs in `runtime/win-x64`; the installer
also carries `LICENSES.md`. See `docs/licenses.md` for the complete matrix and obligations.

## Models
- **NVIDIA Parakeet TDT 0.6B v3** — © NVIDIA. Licence: **CC-BY-4.0**.
  https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
  ONNX conversion by Ilya Stupakov (CC-BY-4.0), https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx
  A Qualcomm QNN context binary for Snapdragon X2 (HTP V81) is derived from these weights by
  OwlWhisp and remains under CC-BY-4.0.
- **Silero VAD** — © Silero Team. Licence: **MIT**. https://github.com/snakers4/silero-vad
- Test fixtures: **FLEURS** (Conneau et al., 2022), **CC-BY-4.0**.

## Inference runtime
- **ONNX Runtime** — © Microsoft. Licence: **MIT**.
- **ONNX Runtime CUDA execution provider** (Windows x64 `.dll` and Linux ARM64 `.so`)
  - Copyright Microsoft. Licence: **MIT**. The matching GPU core ships with it. Linux ARM64 uses
  the pinned 1.30.0 official GPU wheel's native C libraries and carries its matching MIT licence
  and third-party notices. System NVIDIA driver/CUDA/cuDNN libraries are not bundled on Linux.
- **ONNX Runtime TensorRT execution provider** (`onnxruntime_providers_tensorrt.dll`, Windows x64)
  — © Microsoft. Licence: **MIT**.
- **NVIDIA TensorRT 10 runtime DLLs** (Windows x64) — © NVIDIA. Subject to the
  [TensorRT SDK agreement](https://docs.nvidia.com/deeplearning/tensorrt/latest/reference/sla.html),
  whose supplement identifies runtime DLLs as distributable. The `NtvLibs` NuGet wrapper licence
  covers only wrapper content, not these libraries.
- **Microsoft Windows ML and DirectML runtime** (Windows x64) — © Microsoft. The package licence
  travels with the separate DirectML runtime as `windows-ml-license.txt`.
- **NVIDIA CUDA runtime libraries and cuDNN 9** (Windows x64) — © NVIDIA.
  Licences: NVIDIA CUDA Toolkit EULA and NVIDIA cuDNN Software License Agreement. Only
  redistributable runtime DLLs are included in OwlWhisp's private runtime directory; the
  corresponding NVIDIA notices travel beside those DLLs.
- **ONNX Runtime QNN execution provider** (`onnxruntime_providers_qnn.dll`) — © Qualcomm
  Technologies, Inc. Licence: **MIT**.
- **Qualcomm AI Engine Direct (QNN/QAIRT) runtime libraries** — © Qualcomm Technologies, Inc.
  Licence: **Qualcomm AI Stack License** (proprietary; `LicenseRef-Qualcomm-AI-Stack-License`).
  Redistributed in object-code form only, incorporated in this application, per that licence;
  `Qualcomm_LICENSE.pdf` is shipped alongside the binaries.

## Frameworks & libraries (all MIT and/or Apache-2.0 unless noted)
iced and its stack (MIT), winit (Apache-2.0), tiny-skia (BSD-3-Clause), tray-icon and muda
(MIT/Apache-2.0); the `ort`/`ort-sys` crates; cpal, rubato, hound,
realfft, rustfft, ndarray, half; enigo, arboard, global-hotkey, handy-keys; keyring (+ `clipboard-win`
under BSL-1.0 on Windows, via arboard); reqwest (native-TLS / `schannel` MIT), tokio, futures, serde,
serde_json, thiserror, anyhow, tracing, sha2, hex, directories, sysinfo, regex, unicode-normalization,
zip, tar, bzip2, flate2, clap, indicatif, uuid, chrono, parking_lot, crossbeam-channel, tokio-util;
None. The user interface is Rust; there is no JavaScript in a shipped build.

`option-ext` 0.2.0 is **MPL-2.0** and is linked in through `directories`. Its notice and a pointer
to its source must ship with a binary; see `docs/licenses.md`.

No GPL/AGPL/LGPL code is linked into the **default** OwlWhisp build.

**Optional `sherpa` engine:** builds made with `--features sherpa` link the `sherpa-onnx` native
library. Its *default* prebuilt archive statically includes **espeak-ng (GPL-3.0-or-later)**, which
is incompatible with the proprietary Qualcomm QNN runtime this app also ships — such a build must
not be distributed. Use a `-no-tts` sherpa-onnx archive (see `docs/licenses.md`) and re-check this
file before releasing any binary with that feature enabled. Components then added are permissive:
sherpa-onnx (Apache-2.0, © k2-fsa), kaldi-native-fbank (Apache-2.0), kissfft (BSD-3-Clause).
