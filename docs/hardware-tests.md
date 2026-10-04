# Simulated hardware tests

```sh
cargo test -p lw-app --test hardware_matrix
```

The suite runs without a staged ONNX Runtime, downloaded models, administrator rights, network
access or a GPU. It uses the same detection and readiness functions as the application, supplying
OS observations instead of querying native APIs. It does not change environment variables, the
registry, actual drivers or the user's settings.

## Coverage

`tests/fixtures/hardware/configurations.json` contains 45 named configurations with explicit
expected hardware and installation actions:

- Windows, macOS and Linux, each with x64 and ARM64 process architectures.
- Intel, AMD, Qualcomm, Apple Silicon and generic ARM CPUs; headless and unidentified machines.
- NVIDIA SM 75/80/86/89/90/120, older unsupported CUDA hardware, unknown future TensorRT
  partitions, missing/old drivers, integrated plus discrete graphics, multiple NVIDIA GPUs and headless driver-API enumeration.
- NVIDIA GB10 / DGX Spark on Linux ARM64, including headless driver detection and an older driver.
  These observations do not claim successful CUDA inference on a real Spark.
- AMD Radeon, Intel integrated/Arc, Qualcomm Adreno and Apple GPUs.
- Qualcomm NPU V73/V81, absent NPU drivers and stale Qualcomm driver packages on unrelated
  hardware or in an x64 process. Intel/AMD NPU relevance is distinguished from model support.
- Software rendering, remote display adapters, unsupported OS/architecture and failed probes.

Each fixture checks all nine accelerator types. Additional cases cover Windows registry CPU
precedence, six runtime directory names, library extensions, exact pinned package IDs, PCI driver
targets, provider presence/registration/device counts/errors, absent model artifacts and failed
model checks. Windows DriverStore and Linux DRM sysfs trees are built in temporary directories
and parsed on every test host, including Windows.

The readiness policy lives in `lw-app::accelerators`; Settings calls that policy directly.
`Platform::current()` and OS probes are adapters around the same explicit-input policies exercised
by the tests. A physically detected GPU does not imply an installed or usable provider.
`Need additional action` must have a concrete action; unsupported combinations remain unavailable.
Automatic runtime downloads currently have pinned packages for Windows x64 only. Provider support
on another target is separate from package availability and actual successful device enumeration.

## CI and limits

The matrix is included automatically in `cargo test --workspace` on every existing CI runner:
Windows, macOS and Linux, each on x64 and ARM64. All six simulated targets run on each host. To add a
configuration, add a named fixture with its expected results; failures identify the fixture and
accelerator. New GPU partitions must also have a pinned package before expecting an install action.

These tests emulate **observations and decisions**, not a GPU, kernel or native driver ABI. They
cannot prove driver loading, model compatibility, accuracy or inference speed on an untested
card. Those require native hardware tests, such as the ignored ORT/model tests and
`scripts/runtime/verify-addons.py`. Native CI additionally compiles and tests all six targets; that does not replace actual GPU/NPU
inference and performance checks on the corresponding devices.
