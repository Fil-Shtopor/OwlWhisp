# OwlWhisp 0.1.5 preview

This release repairs dictation when a saved GPU preference is unavailable, restores a stable
model list and adds an experimental Windows ARM64 GPU path for NVIDIA RTX Spark.

- **Dictation recovery:** if a saved TensorRT/CUDA/DirectML/WebGPU choice cannot initialize,
  dictation retries the model on CPU and displays the original failure in the active-backend
  notes. The saved accelerator preference is preserved. A failed load is retried on the next
  utterance, so repairing an installation no longer leaves the worker stuck on a cached error.
  Explicit benchmarks remain strict; a failed GPU run is never reported as a GPU measurement.
- **Windows GPU runtime loading:** a capability probe no longer redirects DLL dependency lookup
  from an installed TensorRT/CUDA runtime back to the smaller base runtime. Provider workers
  keep their own runtime override, and repeated ONNX Runtime initialization preserves the active
  DLL directory. This fixes the cuDNN process failure observed after restoring TensorRT libraries.
- **Model order:** NVIDIA stays first, with Parakeet TDT 0.6B v3 before the 110M model. Entries
  retain their catalog order within vendor groups. Hardware recommendations, installation and
  selection no longer move rows or vendor groups.
- **RTX Spark / Windows ARM64:** native DirectML is now bundled alongside WebGPU. Both providers
  run in isolated workers and can be repaired with architecture-specific, hash-pinned downloads.
  Select Parakeet TDT 0.6B v3 and DirectML or WebGPU, prepare its GPU encoder in Settings, then
  compare accelerators in Benchmark. **Experimental, not tested on a physical RTX Spark.** The
  warning appears in Settings and the README. CUDA and classic TensorRT add-ons remain Windows
  x64 only; Windows ARM64 support uses DirectML/WebGPU.
- ARM64 packaging requires the DirectML core, provider dependencies and licence, verifies every
  native binary's architecture, and loads the packaged CPU runtime on the native CI runner.
  Simulated hardware tests include RTX Spark/N1X and generic ARM CPUs with NVIDIA PCI devices.
- Includes the start-minimized-to-tray option and Linux ARM64 DGX Spark CUDA preview from 0.1.4.

GPU/NPU acceleration applies to Parakeet; other catalog models run on CPU where sherpa is included.
macOS/Linux desktop integration remains in preview. Windows installers and macOS bundles remain
unsigned. [RTX Spark setup](https://github.com/Fil-Shtopor/OwlWhisp/blob/v0.1.5/docs/build.md#nvidia-rtx-spark-windows-arm64).
