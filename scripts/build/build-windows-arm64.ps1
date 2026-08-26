# Build LocalWisper for Windows 11 ARM64 (native aarch64-pc-windows-msvc).
#
# Prerequisites:
#   - Rust stable with the aarch64-pc-windows-msvc host toolchain
#   - MSVC ARM64 toolset + Windows SDK (or the portable toolset env script)
#   - Node.js 18+ and npm (for the Tauri frontend)
#   - The runtime DLLs staged in runtime\win-arm64 (see scripts\runtime\fetch-runtime.ps1)
#
# Usage:  pwsh -File scripts\build\build-windows-arm64.ps1 [-Release] [-Cli] [-App]

param(
    [switch]$Release = $true,
    [switch]$Cli,
    [switch]$App
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

if (-not $Cli -and -not $App) { $Cli = $true; $App = $true }

if ($Cli) {
    Write-Host "== Building lw-cli ($target) =="
    cargo build -p lw-cli $profileFlag --target $target
}

if ($App) {
    Write-Host "== Building the frontend =="
    Push-Location app\frontend
    npm install
    npm run build
    Pop-Location

    Write-Host "== Building the Tauri app ($target) =="
    # tauri-cli picks up the target; NSIS bundle is produced under target\$target\release\bundle
    npx --yes @tauri-apps/cli@2 build --target $target
}

Write-Host "Done. Binaries under target\$target\$(if ($Release) {'release'} else {'debug'})\"
