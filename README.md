# LocalWisper

Local, private, system-wide speech-to-text dictation for Windows 11 ARM64 (Snapdragon X2 Elite first),
macOS (Apple Silicon) and Linux — built around a modular, hardware-aware inference core.

* Primary model: NVIDIA Parakeet TDT 0.6B v3 (25 European languages, punctuation + capitalization)
* Rust core (audio, VAD, inference orchestration, text pipeline, platform integration) + Tauri 2 shell + TypeScript UI
* Backends are pluggable: Parakeet on Qualcomm Hexagon NPU (QNN/HTP), Parakeet on CPU (ONNX Runtime),
  Whisper, CoreML engines on macOS — selected automatically, always reported honestly in Diagnostics.

Status, build instructions, backend matrix and honest limitations: see `docs/FINAL_REPORT.md`.
Documentation index: `docs/README.md`.

License: Apache-2.0 (application code). Models and redistributed runtimes have their own licenses — see `docs/licenses.md`.
