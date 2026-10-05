# OwlWhisp 0.1.7 preview

Fixes Qualcomm NPU setup when Diagnostics reports a usable Snapdragon NPU but Settings says
Unavailable for the installed Parakeet model.

- **Prepare NPU model:** Settings now shows **Need additional action** with a working button
  when QNN is usable but the encoder is missing. It downloads the pinned FP32 encoder and
  external weights (about 2.5 GB), verifies their hashes and checks the requested NPU on real
  speech before selecting it. Failed checks keep the error and a retry button.
- **Native static graph preparation:** the ordinary FP32 encoder has dynamic shapes, which
  QNN cannot compile directly. OwlWhisp fixes the batch and 20-second window dimensions and
  folds shape calculations on CPU before preparing the Hexagon context. No Python installation
  is required. Prepared graphs and NPU contexts are cached for subsequent use. First preparation
  can take several minutes and needs additional disk space for the graph weights and context.
- **Consistent file checks:** Settings and the engine use the same encoder requirements.
  A graph without its external weights no longer enables the NPU. Existing static encoders
  remain supported. An unusable QNN device is rejected before expensive model preparation.
- Diagnostics describes provider/device readiness; Settings additionally describes whether the
  selected model is ready. The NPU Info panel explains this distinction and the setup steps.

Validation: full Rust workspace tests, clippy and Python packaging tests passed on Windows x64.
Preparation was tested with the actual pinned Parakeet FP32 encoder: static/dynamic output cosine
similarity exceeded 0.99999999998. Native release runners additionally test dimension fixing,
serialization and inference on all six platforms, including Windows ARM64.

This fix has not yet been exercised on a physical Snapdragon in this release cycle. Successful
setup requires a compatible Snapdragon, OEM NPU driver and bundled QNN runtime; the app checks
the actual requested NPU and does not count CPU fallback as a successful setup. Other models
remain CPU-only. Existing unsigned preview and RTX Spark experimental limitations still apply.
