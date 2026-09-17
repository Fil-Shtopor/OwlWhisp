# Build LocalWisper for Windows 11 ARM64 (native aarch64-pc-windows-msvc).
#
# Prerequisites:
#   - Rust stable with the aarch64-pc-windows-msvc host toolchain
#   - MSVC ARM64 toolset + Windows SDK (or the portable toolset env script)
#   - Node.js 18+ and npm (for the Tauri frontend)
#   - clang on PATH, unless -NoSherpa: a build-dependency of sherpa-onnx-sys (ring, for its
#     HTTPS fetch) has no assembly path for aarch64-windows that MSVC alone can build
#   - The runtime DLLs staged in runtime\win-arm64 (see scripts\runtime\fetch-runtime.ps1)
#
# Usage:  pwsh -File scripts\build\build-windows-arm64.ps1 [-Release] [-Cli] [-App] [-NoSherpa]

#
# The portable `sherpa` engine is built in by default, because 14 of the 15 catalog models need it
# and a user who installs one expects it to run. It is NOT a default *cargo* feature: a plain
# `cargo build` with it on would let sherpa-onnx-sys download its stock archive, which statically
# links GPL-3.0 espeak-ng and cannot ship beside the proprietary Qualcomm runtime. This script
# stages the GPL-free `-no-tts` libraries first (fetch-sherpa.ps1 refuses if that archive ever
# carries a real espeak-ng again) and only then turns the feature on. `-NoSherpa` builds the
# Parakeet-only binary.

param(
    [switch]$Release = $true,
    [switch]$Cli,
    [switch]$App,
    [switch]$NoSherpa
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root

# If the portable MSVC ARM64 env script is present, source it (dev machines without a full VS).
$envPs1 = Join-Path $env:USERPROFILE ".msvc-arm64\env-arm64.ps1"
if (Test-Path $envPs1) {
    Write-Host "Sourcing portable MSVC ARM64 environment"
    . $envPs1
}

$profileFlag = if ($Release) { "--release" } else { "" }
$target = "aarch64-pc-windows-msvc"

# Stage the GPL-free sherpa-onnx libraries and point the build at them. SHERPA_ONNX_LIB_DIR is what
# stops sherpa-onnx-sys fetching its own (GPL) archive, so it must be set before any cargo call.
$featureArgs = @()
if (-not $NoSherpa) {
    # ring (a build-dependency of sherpa-onnx-sys, three levels down) needs clang on aarch64
    # Windows. Say so here rather than letting cc-rs fail 200 lines into a dependency nobody
    # asked for, with a message that names neither sherpa nor this switch.
    if (-not (Get-Command clang -ErrorAction SilentlyContinue)) {
        throw "clang is not on PATH, and the ``sherpa`` engine needs it to build (ring, a build-dependency of sherpa-onnx-sys). Install LLVM and put its bin directory on PATH, or pass -NoSherpa to build the Parakeet-only binary."
    }
    Write-Host "== Staging GPL-free sherpa-onnx libraries ==" -ForegroundColor Cyan
    $env:SHERPA_ONNX_LIB_DIR = & pwsh -NoProfile -File (Join-Path $PSScriptRoot "fetch-sherpa.ps1") -Platform "win-arm64" -Quiet
    if (-not $env:SHERPA_ONNX_LIB_DIR -or -not (Test-Path $env:SHERPA_ONNX_LIB_DIR)) {
        throw "fetch-sherpa.ps1 did not produce a library directory; refusing to build with the -no-tts libraries unverified"
    }
    Write-Host "SHERPA_ONNX_LIB_DIR = $env:SHERPA_ONNX_LIB_DIR"
    $featureArgs = @("--features", "sherpa")
} else {
    Write-Host "== Sherpa engine off (-NoSherpa): only parakeet-tdt-0.6b-v3 will run ==" -ForegroundColor Yellow
}

if (-not $Cli -and -not $App) { $Cli = $true; $App = $true }

if ($Cli) {
    Write-Host "== Building lw-cli ($target) =="
    cargo build -p lw-cli $profileFlag --target $target @featureArgs
}

if ($App) {
    Write-Host "== Building the frontend =="
    Push-Location app\frontend
    npm install
    npm run build
    Pop-Location

    Write-Host "== Building the Tauri app ($target) =="
    # The CLI passes --features custom-protocol itself (a plain `cargo build` would not,
    # and the app would try to load the Vite dev server instead of the embedded assets).
    # tauri-cli picks up the target; NSIS bundle is produced under target\$target\release\bundle
    npx --yes @tauri-apps/cli@2 build --target $target @featureArgs
}

Write-Host "Done. Binaries under target\$target\$(if ($Release) {'release'} else {'debug'})\"
