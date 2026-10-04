# OwlWhisp 0.1.6 preview

This release fixes the multilingual Fast recommendation and installed benchmark comparisons.

- **Fast + Any:** recommends NVIDIA Parakeet TDT 0.6B v3, favoring broad language coverage among
  models tagged Fast. Previously, a coarse CPU speed estimate selected the five-language
  SenseVoice specialist. Choosing a specific language still ranks matching models by estimated
  speed. Unavailable engines are excluded. Benchmark measures actual speed on your computer.
- **Compare all accelerators:** uses the same isolated provider probes as Diagnostics and
  Settings, including separately installed CUDA/TensorRT runtimes. Previously, the sweep only
  considered the base runtime. Each usable provider supported by the selected model is attempted;
  unavailable, unsupported or failed providers have an explicit reason. Strict GPU runs remain
  strict and are never silently replaced by CPU.
- **WER in installed copies:** every package now includes 15 real FLEURS speech excerpts with
  reference transcripts: three each in English, Spanish, Russian, Ukrainian and Chinese. The
  old packages omitted those files and used synthetic audio, which has no reference transcript
  and cannot produce an accuracy score.
- **Mixed languages:** the comparison table shows both WER and CER when both are present, rather
  than reporting "no reference" for a valid mixed-unit measurement. The most-accurate badge uses
  word-weighted WER. Chinese CER is displayed separately and never averaged with WER.
- Package verification requires all reference audio, transcripts and attribution, checks audio
  against the committed fixtures and runs the packaged executable to verify that it can find and
  decode the bundled clips outside the checkout.

Validated on Windows x64 with an RTX 4080 Laptop GPU: CPU, DirectML, WebGPU and a separately
installed TensorRT runtime all completed the comparison and produced WER on the same 12
supported-language clips. CUDA was unavailable without its add-on and was listed with a reason.

WER is an error rate: **lower is better**. A 5% WER means approximately five word errors per 100
reference words. These small samples are useful for local comparisons, not a general accuracy
guarantee. GPU/NPU acceleration applies to Parakeet; other catalog models use CPU where sherpa
is included. Existing RTX Spark experimental/untested labels and unsigned preview limitations
remain unchanged.
