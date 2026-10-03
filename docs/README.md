# OwlWhisp documentation

| Document | What it covers |
|---|---|
| [research.md](research.md) | Phase 0 ecosystem research, the verified X2-NPU result, project/backend comparison, licensing, rejected alternatives |
| [architecture.md](architecture.md) | Crate map, the `SpeechEngine` trait, audio/VAD data flow, platform abstraction, IPC and concurrency model |
| [x2-npu.md](x2-npu.md) | The Snapdragon X2 Elite NPU investigation: exact procedure, what was verified, what remains |
| [benchmarks.md](benchmarks.md) | Measured latency / RTF / WER on the X2E94100, the benchmark methodology, and the estimate-vs-measurement rules behind `lw models` / `lw bench --quick` |
| [using.md](using.md) | Using the desktop app: dictation, hotkey modes (push-to-talk / toggle / hands-free), the model picker, the benchmark panel, logs and file locations |
| [models.md](models.md) | The model catalog: choosing a model for your hardware before downloading, installing it verified, and measuring the real numbers afterwards |
| [hardware.md](hardware.md) | Platforms, installers per OS, and every accelerator: what is verified, what is implemented, and exactly what each remaining one needs |
| [hardware-tests.md](hardware-tests.md) | 42 simulated machine configurations, native CI and the limits of emulation |
| [licenses.md](licenses.md) | Every redistributed component with its licence and obligations |
| [build.md](build.md) | Build matrix and step-by-step build/run instructions per platform |
| [FINAL_REPORT.md](FINAL_REPORT.md) | Honest status: what works, what was tested, what remains |

Experiment logs from the target machine live in [experiments/](experiments/).
