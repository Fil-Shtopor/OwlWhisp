# Build (if needed) and launch the LocalWisper desktop app from a working copy.
#
# Does the four things a fresh checkout needs before the GUI can run:
#   1. builds the frontend into app/frontend/dist (Tauri embeds it at compile time);
#   2. builds the Rust app for the host target;
#   3. makes the ONNX Runtime + QNN DLLs visible next to the executable;
#   4. points the app-data directory at a model directory (link, so nothing is copied).
#
# Usage:
#   pwsh -File scripts\build\run-app.ps1 -ModelDir "C:\path\to\parakeet-tdt-0.6b-v3" [-CacheDir "C:\path\to\qnn-cache"]
#   pwsh -File scripts\build\run-app.ps1 -SkipBuild        # just launch what is already built

param(
    [string]$ModelDir,
    [string]$CacheDir,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root

$envPs1 = Join-Path $env:USERPROFILE ".msvc-arm64\env-arm64.ps1"
if (Test-Path $envPs1) { . $envPs1 }

if (-not $SkipBuild) {
    Write-Host "== frontend ==" -ForegroundColor Cyan
    Push-Location app\frontend
    if (-not (Test-Path node_modules)) { npm install }
    npm run build
    Pop-Location

    Write-Host "== app (release) ==" -ForegroundColor Cyan
    # `custom-protocol` embeds the frontend; without it Tauri loads build.devUrl and the
    # window shows "localhost refused to connect".
    cargo build -p localwisper --release --features custom-protocol
}

$exeDir = Join-Path $root "target\release"
$exe = Join-Path $exeDir "localwisper.exe"
if (-not (Test-Path $exe)) { throw "not built: $exe" }

# 3. Runtime DLLs beside the executable (the app looks in <exe dir>\runtime\win-arm64).
$runtimeSrc = Join-Path $root "runtime"
$runtimeLink = Join-Path $exeDir "runtime"
if ((Test-Path (Join-Path $runtimeSrc "win-arm64\onnxruntime.dll")) -and -not (Test-Path $runtimeLink)) {
    cmd /c mklink /J "$runtimeLink" "$runtimeSrc" | Out-Null
}
if (-not (Test-Path (Join-Path $runtimeLink "win-arm64\onnxruntime.dll"))) {
    Write-Warning "ONNX Runtime not staged - run scripts\runtime\fetch-runtime.ps1 first (the app will report an engine error)."
}

# 4. Model + optional QNN context cache in the app-data directory.
$appData = Join-Path $env:APPDATA "ai.localwisper.app"
New-Item -ItemType Directory -Force -Path $appData | Out-Null
if ($ModelDir) {
    if (-not (Test-Path $ModelDir)) { throw "model directory not found: $ModelDir" }
    $models = Join-Path $appData "models"
    New-Item -ItemType Directory -Force -Path $models | Out-Null
    $target = Join-Path $models (Split-Path $ModelDir -Leaf)
    if (-not (Test-Path $target)) { cmd /c mklink /J "$target" "$ModelDir" | Out-Null }
    Write-Host "model: $target"
}
if ($CacheDir -and (Test-Path $CacheDir)) {
    $cacheLink = Join-Path $appData "cache"
    if (-not (Test-Path $cacheLink)) { cmd /c mklink /J "$cacheLink" "$CacheDir" | Out-Null }
    Write-Host "qnn cache: $cacheLink"
}

Write-Host "== launching ==" -ForegroundColor Green
Write-Host "  hold Ctrl+Alt+Space to dictate; tray icon -> Settings / Diagnostics / Quit"
Start-Process -FilePath $exe -WorkingDirectory $exeDir
