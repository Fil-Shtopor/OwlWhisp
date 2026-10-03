# Fetch and stage the native ONNX Runtime plus execution-provider libraries into
# runtime\<platform> (the set OwlWhisp ships and loads at runtime).
#
# Sources (all redistributable - see docs\licenses.md):
#   - onnxruntime-<platform>                    official ORT release, MIT
#   - Microsoft.ML.OnnxRuntime.EP.WebGpu        MIT; portable GPU provider (D3D12 / Vulkan / Metal)
#   - Microsoft.ML.OnnxRuntime.Gpu.Windows      MIT; matched CUDA provider and ORT core on win-x64
#   - NVIDIA CUDA/cuDNN redistributables        NVIDIA licence; private runtime DLLs on win-x64
#   - Qualcomm.ML.OnnxRuntime.QNN               MIT EP + Qualcomm AI Stack License for the QNN libs
#
# The QNN libraries are Windows-on-Snapdragon only and are governed by a proprietary licence that
# forbids shipping them as a standalone download, so they are staged only for win-arm64 and the
# licence PDF travels with them.
#
# Windows x64 also stages a separate Windows ML core for DirectML and TensorRT 10 libraries
# for Ada (SM 8.9). The DirectML core runs in a worker process because ORT is process-global.
#
# Usage:
#   pwsh -File scripts\runtime\fetch-runtime.ps1
#   pwsh -File scripts\runtime\fetch-runtime.ps1 -Platform win-x64
#   pwsh -File scripts\runtime\fetch-runtime.ps1 -Platform osx-arm64 -SkipQnn

param(
    [ValidateSet("win-arm64", "win-x64", "osx-arm64", "osx-x64", "linux-x64")]
    [string]$Platform = "",
    [string]$OrtVersion = "1.28.1",
    [string]$QnnVersion = "2.5.0",
    [string]$WebGpuVersion = "0.3.0",
    [string]$GpuVersion = "1.28.0",
    [string]$WindowsMlVersion = "2.5.77-rc",
    [string]$TensorRtVersion = "10.14.1.48",
    [switch]$SkipQnn,
    [switch]$SkipWebGpu,
    [switch]$SkipCuda,
    [switch]$SkipCudaDeps,
    [switch]$SkipDirectMl,
    [switch]$SkipTensorRt
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
    "osx-x64"    { "onnxruntime-osx-x64-$OrtVersion" }
    "linux-x64"  { "onnxruntime-linux-x64-$OrtVersion" }
}
$ortExt = if ($Platform -like "win-*") { "zip" } else { "tgz" }
$ortArchive = Join-Path $tmp "ort.$ortExt"
if (-not (Test-Path $ortArchive)) {
    Invoke-WebRequest -Uri "https://github.com/microsoft/onnxruntime/releases/download/v$OrtVersion/$ortName.$ortExt" -OutFile "$ortArchive.partial"
    Move-Item "$ortArchive.partial" $ortArchive
}
$ortDir = Join-Path $tmp "ort"
New-Item -ItemType Directory -Force -Path $ortDir | Out-Null
if ($ortExt -eq "zip") { Expand-Archive -Force $ortArchive $ortDir } else { tar -xzf $ortArchive -C $ortDir }
Get-ChildItem -Recurse $ortDir -Include "onnxruntime*.dll", "libonnxruntime*.so*", "libonnxruntime*.dylib" |
    ForEach-Object { Copy-Item $_.FullName $dest -Force }

# The ordinary ORT release is CPU-only. The GPU NuGet contains a matched core DLL and the
# legacy CUDA provider; both must come from the same package. A driver or CUDA Toolkit alone
# cannot supply either file. Keep ARM64 on the QNN build: NVIDIA's GPU package is x64 only.
if ($Platform -eq "win-x64" -and -not $SkipCuda) {
    Write-Host "== Microsoft.ML.OnnxRuntime.Gpu.Windows $GpuVersion (CUDA 13 / cuDNN 9) =="
    $gpuPackage = Join-Path $tmp "ort-gpu.zip"
    if (-not (Test-Path $gpuPackage)) {
        Invoke-WebRequest -Uri "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.gpu.windows/$GpuVersion/microsoft.ml.onnxruntime.gpu.windows.$GpuVersion.nupkg" -OutFile "$gpuPackage.partial"
        Move-Item "$gpuPackage.partial" $gpuPackage
    }
    $gpuDir = Join-Path $tmp "ort-gpu"
    Expand-Archive -Force $gpuPackage $gpuDir
    $gpuNative = Join-Path $gpuDir "runtimes\win-x64\native"
    foreach ($file in @("onnxruntime.dll", "onnxruntime_providers_shared.dll", "onnxruntime_providers_cuda.dll")) {
        $source = Join-Path $gpuNative $file
        if (-not (Test-Path $source)) { throw "GPU package $GpuVersion is missing $file" }
        Copy-Item $source $dest -Force
    }
    Copy-Item (Join-Path $gpuDir "LICENSE") (Join-Path $dest "onnxruntime-gpu-LICENSE.txt") -Force
    Copy-Item (Join-Path $gpuDir "ThirdPartyNotices.txt") (Join-Path $dest "onnxruntime-gpu-ThirdPartyNotices.txt") -Force
    Write-Host "CUDA requires the NVIDIA CUDA 13 and cuDNN 9 runtime DLLs on the machine."

    if (-not $SkipCudaDeps) {
        # Only the redistributable runtime DLLs are shipped. They live beside the matched
        # ONNX Runtime DLLs in OwlWhisp's private runtime directory, with NVIDIA's notices.
        $redists = @(
            @("cuda", "cuda_cudart", "cuda_cudart-windows-x86_64-13.0.88-archive.zip", "cudart64_13.dll"),
            @("cuda", "libcublas", "libcublas-windows-x86_64-13.0.2.14-archive.zip", "cublas64_13.dll", "cublasLt64_13.dll"),
            @("cuda", "libcufft", "libcufft-windows-x86_64-12.0.0.61-archive.zip", "cufft64_12.dll"),
            @("cuda", "libcurand", "libcurand-windows-x86_64-10.4.0.35-archive.zip", "curand64_10.dll"),
            @("cudnn", "cudnn", "cudnn-windows-x86_64-9.13.1.26_cuda13-archive.zip", "cudnn64_9.dll", "cudnn_graph64_9.dll", "cudnn_ops64_9.dll", "cudnn_heuristic64_9.dll", "cudnn_adv64_9.dll", "cudnn_cnn64_9.dll", "cudnn_engines_precompiled64_9.dll", "cudnn_engines_runtime_compiled64_9.dll")
        )
        foreach ($redist in $redists) {
            $family, $component, $archiveName = $redist[0..2]
            $baseUrl = if ($family -eq "cudnn") { "https://developer.download.nvidia.com/compute/cudnn/redist" } else { "https://developer.download.nvidia.com/compute/cuda/redist" }
            $archive = Join-Path $tmp $archiveName
            $extract = Join-Path $tmp ([IO.Path]::GetFileNameWithoutExtension($archiveName))
            Write-Host "== NVIDIA $component =="
            if (-not (Test-Path $archive)) {
                Invoke-WebRequest -Uri "$baseUrl/$component/windows-x86_64/$archiveName" -OutFile "$archive.partial"
                Move-Item "$archive.partial" $archive
            }
            Expand-Archive -Force $archive $extract
            foreach ($file in $redist[3..($redist.Length - 1)]) {
                $matches = @(Get-ChildItem $extract -Recurse -File -Filter $file)
                if ($matches.Count -ne 1) { throw "$archiveName must contain exactly one $file; found $($matches.Count)" }
                Copy-Item $matches[0].FullName $dest -Force
            }
            Get-ChildItem $extract -Recurse -File | Where-Object { $_.Name -match '^(LICENSE|EULA)(\.|$)' } |
                ForEach-Object { Copy-Item $_.FullName (Join-Path $dest "nvidia-$component-$($_.Name)") -Force }
        }
    }

    if (-not $SkipTensorRt) {
        # The ORT GPU package already contains the legacy TensorRT EP. Its DLL has no plugin
        # CreateEpFactories entry point; the application registers it as a session EP instead.
        Copy-Item (Join-Path $gpuNative "onnxruntime_providers_tensorrt.dll") $dest -Force
        $trtComponents = @("nvinfer_10", "nvinfer_plugin_10", "nvonnxparser_10", "nvinfer_builder_resource_sm89_10")
        foreach ($component in $trtComponents) {
            $id = "ntvlibs.tensorrt.cuda13.$component.runtime.win-x64"
            $archive = Join-Path $tmp "$id.$TensorRtVersion.zip"
            if (-not (Test-Path $archive)) {
                Invoke-WebRequest -UseBasicParsing -Uri "https://api.nuget.org/v3-flatcontainer/$id/$TensorRtVersion/$id.$TensorRtVersion.nupkg" -OutFile "$archive.partial"
                Move-Item "$archive.partial" $archive
            }
            $extract = Join-Path $tmp "$id-extracted"
            Expand-Archive -Force $archive $extract
            $dll = Join-Path $extract "runtimes\win-x64\native\$component.dll"
            if (-not (Test-Path $dll)) { throw "$id is missing $component.dll" }
            Copy-Item $dll $dest -Force
            if ($component -eq "nvinfer_10") {
                Copy-Item (Join-Path $extract "LICENSE.txt") (Join-Path $dest "ntvlibs-tensorrt-LICENSE.txt") -Force
            }
        }
        Write-Host "TensorRT 10 staged for NVIDIA Ada SM 8.9; other GPU architectures need their matching builder resource."
    }

    if (-not $SkipDirectMl) {
        $id = "microsoft.windows.ai.machinelearning"
        $archive = Join-Path $tmp "$id.$WindowsMlVersion.zip"
        if (-not (Test-Path $archive)) {
            Invoke-WebRequest -UseBasicParsing -Uri "https://api.nuget.org/v3-flatcontainer/$id/$WindowsMlVersion/$id.$WindowsMlVersion.nupkg" -OutFile "$archive.partial"
            Move-Item "$archive.partial" $archive
        }
        $extract = Join-Path $tmp "windows-ml"
        Expand-Archive -Force $archive $extract
        $directMlDest = Join-Path $root "runtime\win-x64-directml"
        New-Item -ItemType Directory -Force -Path $directMlDest | Out-Null
        foreach ($file in @("onnxruntime.dll", "DirectML.dll", "Microsoft.Windows.AI.MachineLearning.dll")) {
            $source = Join-Path $extract "runtimes\win-x64\native\$file"
            if (-not (Test-Path $source)) { throw "Windows ML $WindowsMlVersion is missing $file" }
            Copy-Item $source $directMlDest -Force
        }
        Copy-Item (Join-Path $extract "license.txt") (Join-Path $directMlDest "windows-ml-license.txt") -Force
    }
}

# --- WebGPU plugin EP (portable GPU support, every platform) ------------------------------------
if (-not $SkipWebGpu) {
    Write-Host "== Microsoft.ML.OnnxRuntime.EP.WebGpu $WebGpuVersion ($Platform) =="
    $pkg = Join-Path $tmp "webgpu.zip"
    Invoke-WebRequest -Uri "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.ep.webgpu/$WebGpuVersion/microsoft.ml.onnxruntime.ep.webgpu.$WebGpuVersion.nupkg" -OutFile $pkg
    $wg = Join-Path $tmp "webgpu"
    Expand-Archive -Force $pkg $wg
    $native = Join-Path $wg "runtimes\$Platform\native"
    if (Test-Path $native) {
        Get-ChildItem $native -File | Where-Object {
            $_.Name -notin @("onnxruntime.dll", "onnxruntime_providers_shared.dll")
        } | ForEach-Object { Copy-Item $_.FullName $dest -Force }
        $lic = Join-Path $wg "LICENSE"
        if (Test-Path $lic) { Copy-Item $lic (Join-Path $dest "onnxruntime-webgpu-LICENSE.txt") -Force }
    } else {
        Write-Warning "WebGPU EP has no build for $Platform; GPU support will be unavailable there."
    }
}

# --- Qualcomm QNN (Windows on Snapdragon only) --------------------------------------------------
if (-not $SkipQnn -and $Platform -eq "win-arm64") {
    Write-Host "== Qualcomm.ML.OnnxRuntime.QNN $QnnVersion (win-arm64 native) =="
    $nupkg = Join-Path $tmp "qnn.zip"
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
Write-Host "Verify what OwlWhisp can actually use here with:  lw diagnose"
if ($Platform -like "win-*") {
    $expect = if ($Platform -eq "win-arm64") { "0xAA64 (ARM64)" } else { "0x8664 (x64)" }
    Write-Host "NOTE: every DLL must be machine $expect or the app will fail to load it."
}
