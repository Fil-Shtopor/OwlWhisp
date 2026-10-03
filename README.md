# OwlWhisp

<p align="center">
  <img src="assets/icons/icon-256.png" width="180" alt="OwlWhisp owl and voice-wave icon">
</p>

**Your voice stays yours.** OwlWhisp is private, local, on-device speech-to-text for Windows,
macOS, and Linux. It listens on your machine, transcribes locally, and places the result into the
active application. No cloud account and no browser engine are required.

## What it offers

- **Private by design:** microphone audio and models stay on the device.
- **Fast local inference:** Parakeet runs on CPU and, on supported Snapdragon Windows hardware,
  the Qualcomm Hexagon NPU. Other engines remain available for a wider model catalog.
- **Desktop-native:** a Rust application with tray controls, global hotkeys, diagnostics, and no
  webview process.
- **Open source:** Apache-2.0 application code with third-party notices included in every package.

## Downloads

Every `v*` tag builds downloadable artifacts in the matching GitHub Release:

| Platform | Download |
|---|---|
| Windows ARM64 / x64 | NSIS installer and portable ZIP |
| macOS Apple Silicon / Intel | `.app` bundle in a ZIP |
| Linux x64 / ARM64 | portable `.tar.gz` |

Release files include a per-platform `SHA256SUMS-*.txt`. Windows installers are unsigned until code-signing is
configured; macOS packages need signing and notarization before Gatekeeper will treat them as
identified-developer applications.

## Build from source

```powershell
pwsh -File scripts/runtime/fetch-runtime.ps1
cargo build --release -p lw-gui --features sherpa
```

Run `target/release/owlwhisp` (or `owlwhisp.exe` on Windows). For platform-specific build,
runtime, and packaging instructions, see [docs/build.md](docs/build.md). The documentation index
is in [docs/README.md](docs/README.md).

## Licensing

Application code is **Apache-2.0**. Model and runtime licences are documented in
[docs/licenses.md](docs/licenses.md) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
