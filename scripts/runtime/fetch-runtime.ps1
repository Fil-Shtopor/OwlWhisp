# Fetch and stage the native ARM64 ONNX Runtime + Qualcomm QNN execution-provider DLLs into
# runtime\win-arm64 (the set LocalWisper ships / loads at runtime).
#
# Sources (both redistributable — see docs\licenses.md):
#   - onnxruntime-win-arm64  (official ORT release, MIT)
#   - Qualcomm.ML.OnnxRuntime.QNN NuGet, runtimes\win-arm64\native (MIT EP + Qualcomm AI Stack
#     License for the QNN libs; the .pdf is shipped alongside)
#
# Usage: pwsh -File scripts\runtime\fetch-runtime.ps1 [-OrtVersion 1.28.1] [-QnnVersion 2.5.0]

param(
    [string]$OrtVersion = "1.28.1",
    [string]$QnnVersion = "2.5.0"
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$dest = Join-Path $root "runtime\win-arm64"
$tmp = Join-Path $env:TEMP "lw-runtime-fetch"
New-Item -ItemType Directory -Force -Path $dest, $tmp | Out-Null

Write-Host "== ONNX Runtime $OrtVersion (win-arm64) =="
$ortZip = Join-Path $tmp "ort.zip"
Invoke-WebRequest -Uri "https://github.com/microsoft/onnxruntime/releases/download/v$OrtVersion/onnxruntime-win-arm64-$OrtVersion.zip" -OutFile $ortZip
Expand-Archive -Force $ortZip (Join-Path $tmp "ort")
Get-ChildItem -Recurse (Join-Path $tmp "ort") -Filter "onnxruntime*.dll" | ForEach-Object {
    Copy-Item $_.FullName $dest -Force
}

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

Write-Host "Staged runtime in $dest :"
Get-ChildItem $dest | Select-Object Length, Name | Format-Table -AutoSize
Write-Host "NOTE: all DLLs must be machine 0xAA64 (native ARM64) for the native app to load them."
