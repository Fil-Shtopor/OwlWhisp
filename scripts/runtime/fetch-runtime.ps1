# Fetch and stage the native ONNX Runtime plus execution-provider libraries into
# runtime\<platform> (the set LocalWisper ships and loads at runtime).
#
# Sources (all redistributable - see docs\licenses.md):
#   - onnxruntime-<platform>                    official ORT release, MIT
#   - Microsoft.ML.OnnxRuntime.EP.WebGpu        MIT; portable GPU provider (D3D12 / Vulkan / Metal)
#   - Qualcomm.ML.OnnxRuntime.QNN               MIT EP + Qualcomm AI Stack License for the QNN libs
#
# The QNN libraries are Windows-on-Snapdragon only and are governed by a proprietary licence that
# forbids shipping them as a standalone download, so they are staged only for win-arm64 and the
# licence PDF travels with them.
#
# Providers this script deliberately does NOT fetch, because they need a vendor SDK installed on
# the machine rather than a redistributable DLL: CUDA / TensorRT (NVIDIA CUDA + cuDNN),
# OpenVINO (Intel OpenVINO runtime), Vitis AI (AMD Ryzen AI SDK). See docs\hardware.md for what
# each one needs; drop their provider DLLs into the same directory and LocalWisper will find them.
#
# Usage:
#   pwsh -File scripts\runtime\fetch-runtime.ps1
#   pwsh -File scripts\runtime\fetch-runtime.ps1 -Platform win-x64
#   pwsh -File scripts\runtime\fetch-runtime.ps1 -Platform osx-arm64 -SkipQnn

param(
    [ValidateSet("win-arm64", "win-x64", "osx-arm64", "linux-x64")]
    [string]$Platform = "",
    [string]$OrtVersion = "1.28.1",
    [string]$QnnVersion = "2.5.0",
    [string]$WebGpuVersion = "0.3.0",
    [switch]$SkipQnn,
    [switch]$SkipWebGpu
)

$ErrorActionPreference = "Stop"

if (-not $Platform) {
    $arch = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq "Arm64") { "arm64" } else { "x64" }
    $Platform = if ($IsMacOS) { "osx-$arch" } elseif ($IsLinux) { "linux-$arch" } else { "win-$arch" }
}

$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$dest = Join-Path $root "runtime\$Platform"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "lw-runtime-fetch\$Platform"
New-Item -ItemType Directory -Force -Path $dest, $tmp | Out-Null
Write-Host "Staging into $dest (platform $Platform)" -ForegroundColor Cyan

# --- ONNX Runtime core ------------------------------------------------------------------------
Write-Host "== ONNX Runtime $OrtVersion ($Platform) =="
$ortName = switch ($Platform) {
    "win-arm64"  { "onnxruntime-win-arm64-$OrtVersion" }
    "win-x64"    { "onnxruntime-win-x64-$OrtVersion" }
    "osx-arm64"  { "onnxruntime-osx-arm64-$OrtVersion" }
    "linux-x64"  { "onnxruntime-linux-x64-$OrtVersion" }
}
$ortExt = if ($Platform -like "win-*") { "zip" } else { "tgz" }
$ortArchive = Join-Path $tmp "ort.$ortExt"
Invoke-WebRequest -Uri "https://github.com/microsoft/onnxruntime/releases/download/v$OrtVersion/$ortName.$ortExt" -OutFile $ortArchive
$ortDir = Join-Path $tmp "ort"
New-Item -ItemType Directory -Force -Path $ortDir | Out-Null
if ($ortExt -eq "zip") { Expand-Archive -Force $ortArchive $ortDir } else { tar -xzf $ortArchive -C $ortDir }
Get-ChildItem -Recurse $ortDir -Include "onnxruntime*.dll", "libonnxruntime*.so*", "libonnxruntime*.dylib" |
    ForEach-Object { Copy-Item $_.FullName $dest -Force }

# --- WebGPU plugin EP (portable GPU support, every platform) ------------------------------------
if (-not $SkipWebGpu) {
    Write-Host "== Microsoft.ML.OnnxRuntime.EP.WebGpu $WebGpuVersion ($Platform) =="
    $pkg = Join-Path $tmp "webgpu.nupkg"
    Invoke-WebRequest -Uri "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.ep.webgpu/$WebGpuVersion/microsoft.ml.onnxruntime.ep.webgpu.$WebGpuVersion.nupkg" -OutFile $pkg
    $wg = Join-Path $tmp "webgpu"
    Expand-Archive -Force $pkg $wg
    $native = Join-Path $wg "runtimes\$Platform\native"
    if (Test-Path $native) {
        Get-ChildItem $native -File | ForEach-Object { Copy-Item $_.FullName $dest -Force }
        $lic = Join-Path $wg "LICENSE"
        if (Test-Path $lic) { Copy-Item $lic (Join-Path $dest "onnxruntime-webgpu-LICENSE.txt") -Force }
    } else {
        Write-Warning "WebGPU EP has no build for $Platform; GPU support will be unavailable there."
    }
}

# --- Qualcomm QNN (Windows on Snapdragon only) --------------------------------------------------
if (-not $SkipQnn -and $Platform -eq "win-arm64") {
    Write-Host "== Qualcomm.ML.OnnxRuntime.QNN $QnnVersion (win-arm64 native) =="
    $nupkg = Join-Path $tmp "qnn.nupkg"
    Invoke-WebRequest -Uri "https://api.nuget.org/v3-flatcontainer/qualcomm.ml.onnxruntime.qnn/$QnnVersion/qualcomm.ml.onnxruntime.qnn.$QnnVersion.nupkg" -OutFile $nupkg
    Expand-Archive -Force $nupkg (Join-Path $tmp "qnn")
    $native = Join-Path $tmp "qnn\runtimes\win-arm64\native"
    $want = @(
        "onnxruntime_providers_qnn.dll", "QnnHtp.dll", "QnnSystem.dll", "QnnHtpPrepare.dll",
        "QnnHtpNetRunExtensions.dll",
        "QnnHtpV81Stub.dll", "libQnnHtpV81Skel.so", "libqnnhtpv81.cat",
        "QnnHtpV73Stub.dll", "libQnnHtpV73Skel.so", "libqnnhtpv73.cat"
    )
    foreach ($f in $want) {
        $src = Join-Path $native $f
        if (Test-Path $src) { Copy-Item $src $dest -Force } else { Write-Warning "missing $f" }
    }
    # Licences (required by the Qualcomm AI Stack License).
    Get-ChildItem -Recurse (Join-Path $tmp "qnn") -Include "LICENSE", "Qualcomm_LICENSE.pdf", "*ThirdParty*" -ErrorAction SilentlyContinue |
        ForEach-Object { Copy-Item $_.FullName (Join-Path $dest ("qnn-" + $_.Name)) -Force }
} elseif (-not $SkipQnn) {
    Write-Host "(skipping QNN: Qualcomm ships it for win-arm64 only)"
}

Write-Host "`nStaged runtime in $dest :" -ForegroundColor Green
Get-ChildItem $dest | Select-Object Length, Name | Format-Table -AutoSize
Write-Host "Verify what LocalWisper can actually use here with:  lw diagnose"
if ($Platform -like "win-*") {
    $expect = if ($Platform -eq "win-arm64") { "0xAA64 (ARM64)" } else { "0x8664 (x64)" }
    Write-Host "NOTE: every DLL must be machine $expect or the app will fail to load it."
}
