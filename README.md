# LocalWisper

Local, private, system-wide speech-to-text dictation for **Windows 11 ARM64 (Snapdragon X2 Elite
first)**, macOS (Apple Silicon) and Linux — built around a modular, hardware-aware inference core.

- **Primary model:** NVIDIA Parakeet TDT 0.6B v3 (25 European languages, punctuation + capitalization).
- **Stack:** Rust, top to bottom. A core (audio, VAD, inference orchestration, text pipeline,
  OS integration), an application layer above it, and a window drawn natively with `iced` on a
  CPU rasteriser — no browser engine, no GPU context, one process. Audio never leaves the core.
- **Backends are pluggable and honest:** Parakeet on the Qualcomm **Hexagon NPU (QNN/HTP V81)**,
  Parakeet on **CPU** (ONNX Runtime), a Whisper adapter, and a macOS CoreML path — selected
  automatically and always reported truthfully in Diagnostics. CPU fallback is never removed.

## Status (verified on a Snapdragon X2 Elite Extreme, Windows 11 ARM64)

- ✅ **Parakeet runs on the X2 Hexagon NPU** via ONNX Runtime's QNN plugin EP — `lw bench`:
  **RTF 0.0145, WER 4.8%** over 12 FLEURS clips (en/ru/es/uk); HTP context binary cached.
- ✅ **CPU fallback** verified independently — **RTF 0.032, WER 5.4%**.
- ✅ **Live microphone** capture → transcribe verified (`lw record`).
- ✅ ~140 unit/integration tests green; full workspace builds; fmt + clippy clean.

Read the honest, detailed status in **[docs/FINAL_REPORT.md](docs/FINAL_REPORT.md)**.

## Quick start (Windows ARM64)

```powershell
pwsh -File scripts\runtime\fetch-runtime.ps1          # stage ORT + QNN EP DLLs (native ARM64)
```
```bash
python scripts/models/download_model.py models/manifests/parakeet-tdt-0.6b-v3.json \
    --dest "$LOCALAPPDATA/LocalWisper/models" --target cpu_int8
cargo build --release -p lw-cli --target aarch64-pc-windows-msvc      # source ~/.msvc-arm64/env-arm64.sh first on the dev box
lw --runtime-dir runtime/win-arm64 diagnose
lw --runtime-dir runtime/win-arm64 transcribe clip.wav --model-dir "$LOCALAPPDATA/LocalWisper/models/parakeet-tdt-0.6b-v3" --backend auto
```

Full build/run instructions: **[docs/build.md](docs/build.md)**. Documentation index:
**[docs/README.md](docs/README.md)**.

## Documentation

| Doc | What |
|---|---|
| [research.md](docs/research.md) | Phase-0 ecosystem research + the verified X2-NPU result |
| [architecture.md](docs/architecture.md) | Crate map, `SpeechEngine` trait, data flow, IPC, concurrency |
| [x2-npu.md](docs/x2-npu.md) | The Snapdragon X2 NPU investigation: exact procedure & caveats |
| [benchmarks.md](docs/benchmarks.md) | Measured latency / RTF / WER on the X2 |
| [licenses.md](docs/licenses.md) | Every redistributed component and its obligations |
| [build.md](docs/build.md) | Build matrix + per-platform build/run |
| [FINAL_REPORT.md](docs/FINAL_REPORT.md) | Honest status: what works, what was tested, what remains |

## Licensing

Application code **Apache-2.0**. NVIDIA Parakeet **CC-BY-4.0** (attributed); ONNX Runtime + QNN EP
MIT; Qualcomm QNN runtime under the Qualcomm AI Stack License (object-code-only, PDF shipped); Silero
VAD MIT. No GPL/AGPL code is linked. See [docs/licenses.md](docs/licenses.md) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
