# Package OwlWhisp for Windows into a portable folder, a zip, and (where the tool is present)
# an NSIS installer.
#
# This replaces the Tauri bundler, which went when the WebView front end did. Nothing here needs a
# toolkit: the application is one executable plus the ONNX Runtime and Qualcomm runtime files it
# loads at startup, and the licence notices that have to travel with them.
#
# Why not `cargo-packager` or `cargo-wix`, which do this for you: `cargo-packager` fails to build on
# this machine because a dependency of a dependency needs clang, which is not installed and which
# the sherpa engine needs too (see build-windows-arm64.ps1). Rather than make clang a prerequisite
# for producing a zip, the staging is done here -- it is thirty lines -- and the installer is left
# to NSIS, which is a single tool with no build step.
#
# Usage:
#   pwsh -File scripts\build\package-windows.ps1 [-Target aarch64-pc-windows-msvc] [-NoInstaller]
#
# Output, under dist\:
#   OwlWhisp-<version>-<target>\        the portable folder; run the exe in place
#   OwlWhisp-<version>-<target>.zip     the same, zipped
#   OwlWhisp-<version>-<target>-setup.exe  only when makensis is on PATH

param(
    [string]$Target = "aarch64-pc-windows-msvc",
    [ValidateSet("win-arm64", "win-x64")]
    [string]$Platform = "",
    [switch]$WithNvidiaRuntime,
    [switch]$RequireInstaller,
    [ValidateSet('None', 'Certificate', 'Azure')]
    [string]$SigningMode = 'None',
    [switch]$RequireSigning,
    [switch]$NoInstaller
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root
if (-not $PSBoundParameters.ContainsKey('SigningMode') -and $env:OWLWHISP_SIGNING_MODE) {
    $SigningMode = $env:OWLWHISP_SIGNING_MODE
}
if ($SigningMode -notin @('None', 'Certificate', 'Azure')) { throw 'Unknown signing mode' }
if ($RequireSigning -and $SigningMode -eq 'None') { throw 'Signing is required but no signing identity is configured' }
$signScript = Join-Path $PSScriptRoot 'sign-windows.ps1'

# The version is read from the workspace manifest rather than typed here, so a release cannot ship
# a folder named after the wrong one.
$manifest = Get-Content (Join-Path $root "Cargo.toml") -Raw
if ($manifest -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
    throw "could not read version from the workspace Cargo.toml"
}
$version = $Matches[1]

$exe = Join-Path $root "target\$Target\release\owlwhisp.exe"
if (-not (Test-Path $exe)) {
    # Fall back to a host-target build, which is what a plain `cargo build --release` produces.
    $exe = Join-Path $root "target\release\owlwhisp.exe"
}
if (-not (Test-Path $exe)) {
    throw "owlwhisp.exe was not found. Build it first: cargo build -p lw-gui --release"
}

$Platform = if ($Platform) { $Platform } elseif ($Target -like "aarch64-*") { "win-arm64" } else { "win-x64" }
$runtime = Join-Path $root "runtime\$Platform"
if (-not (Test-Path $runtime)) {
    throw "runtime\$Platform is missing. Fetch it first: scripts\runtime\fetch-runtime.ps1 -Platform $Platform"
}
# A directory alone is not a usable runtime. Without this DLL the application can still show its
# model catalogue, which made an incomplete installer look like a model problem at first use.
if (-not (Test-Path (Join-Path $runtime "onnxruntime.dll"))) {
    throw "runtime\$Platform\onnxruntime.dll is missing. Fetch the runtime before packaging: scripts\runtime\fetch-runtime.ps1 -Platform $Platform"
}
if ($Platform -eq "win-x64" -and $WithNvidiaRuntime) {
    foreach ($file in @(
        "onnxruntime_providers_cuda.dll", "onnxruntime_providers_shared.dll",
        "cudart64_13.dll", "cublas64_13.dll", "cublasLt64_13.dll",
        "cufft64_12.dll", "curand64_10.dll", "cudnn64_9.dll"
    )) {
        if (-not (Test-Path (Join-Path $runtime $file))) {
            throw "CUDA package is incomplete: runtime\$Platform\$file is missing. Run scripts\runtime\fetch-runtime.ps1 -Platform win-x64."
        }
    }
    foreach ($file in @(
        "onnxruntime_providers_tensorrt.dll", "nvinfer_10.dll",
        "nvinfer_plugin_10.dll", "nvonnxparser_10.dll"
    )) {
        if (-not (Test-Path (Join-Path $runtime $file))) {
            throw "TensorRT package is incomplete: runtime\$Platform\$file is missing. Run scripts\runtime\fetch-runtime.ps1 -Platform win-x64."
        }
    }
}
if ($Platform -eq "win-x64") {
    foreach ($file in @("onnxruntime_providers_webgpu.dll", "dxcompiler.dll", "dxil.dll")) {
        if (-not (Test-Path -LiteralPath (Join-Path $runtime $file))) { throw "Portable GPU runtime is missing $file" }
    }
    $directMlRuntime = Join-Path $root "runtime\win-x64-directml"
    foreach ($file in @("onnxruntime.dll", "DirectML.dll", "Microsoft.Windows.AI.MachineLearning.dll")) {
        if (-not (Test-Path (Join-Path $directMlRuntime $file))) {
            throw "DirectML package is incomplete: runtime\win-x64-directml\$file is missing. Run scripts\runtime\fetch-runtime.ps1 -Platform win-x64."
        }
    }
}

$name = "OwlWhisp-$version-$Target"
$stage = Join-Path $root "dist\$name"
$resolvedStage = [IO.Path]::GetFullPath($stage)
$distRoot = [IO.Path]::GetFullPath((Join-Path $root 'dist')) + [IO.Path]::DirectorySeparatorChar
if (-not $resolvedStage.StartsWith($distRoot, [StringComparison]::OrdinalIgnoreCase)) { throw "Staging path is outside dist" }
if (Test-Path -LiteralPath $resolvedStage) { Remove-Item -LiteralPath $resolvedStage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Write-Host "== Staging $name ==" -ForegroundColor Cyan
Copy-Item $exe (Join-Path $stage "OwlWhisp.exe")
& $signScript -FilePath (Join-Path $stage 'OwlWhisp.exe') -Mode $SigningMode

# The runtime keeps its own folder, because that is where the application looks for it: the loader
# resolves `runtime\<platform>` relative to the executable.
$runtimeDest = Join-Path $stage "runtime\$Platform"
New-Item -ItemType Directory -Force -Path $runtimeDest | Out-Null
if ($Platform -eq "win-x64" -and -not $WithNvidiaRuntime) {
    # CPU and portable GPU support work out of the box. NVIDIA packages are downloaded per GPU.
    foreach ($file in @("onnxruntime.dll", "onnxruntime_providers_shared.dll", "onnxruntime_providers_webgpu.dll", "dxcompiler.dll", "dxil.dll", "onnxruntime-gpu-LICENSE.txt", "onnxruntime-gpu-ThirdPartyNotices.txt", "onnxruntime-webgpu-LICENSE.txt")) {
        $source = Join-Path $runtime $file
        if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination $runtimeDest }
    }
} else {
    Copy-Item -Recurse -Force (Join-Path $runtime "*") $runtimeDest
}
if ($Platform -eq "win-x64") {
    $directMlDest = Join-Path $stage "runtime\win-x64-directml"
    New-Item -ItemType Directory -Force -Path $directMlDest | Out-Null
    Copy-Item -Recurse -Force (Join-Path $directMlRuntime "*") $directMlDest
}

# Model downloads are verified against pinned manifests. They are application data, not part of
# the executable, so the weights stay out of the installer; the manifests must travel with it or
# a fresh installed copy cannot download even the default Parakeet model.
$manifests = Join-Path $root "models\manifests"
if (-not (Test-Path $manifests)) {
    throw "models\manifests is missing; cannot package verified model downloads."
}
$manifestsDest = Join-Path $stage "models\manifests"
New-Item -ItemType Directory -Force -Path $manifestsDest | Out-Null
Copy-Item -Recurse -Force (Join-Path $manifests "*") $manifestsDest

# Licences travel with the binaries they cover. This is not decoration: the ONNX Runtime and
# Qualcomm files are redistributed under terms that require their notices, and `option-ext` is
# MPL-2.0 (see docs\licenses.md).
Copy-Item (Join-Path $root "LICENSE") $stage -ErrorAction SilentlyContinue
Copy-Item (Join-Path $root "docs\licenses.md") (Join-Path $stage "LICENSES.md")
Copy-Item (Join-Path $root "THIRD_PARTY_NOTICES.md") $stage
python (Join-Path $PSScriptRoot 'installer-license.py') --output (Join-Path $stage 'LICENSES.rtf')
if ($LASTEXITCODE -ne 0) { throw 'Could not render installer licence text' }
@{
    version = $version; target = $Target; platform = $Platform
    commit = (git rev-parse HEAD); sherpa = [bool]$env:SHERPA_ONNX_LIB_DIR
    windows_signing = $SigningMode
} | ConvertTo-Json | Set-Content (Join-Path $stage 'BUILD_INFO.json')

$files = (Get-ChildItem -Recurse -File $stage).Count
$bytes = (Get-ChildItem -Recurse -File $stage | Measure-Object -Property Length -Sum).Sum
Write-Host ("staged {0} files, {1:N0} MB" -f $files, ($bytes / 1MB))

Write-Host "== Zipping ==" -ForegroundColor Cyan
$zip = Join-Path $root "dist\$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip
Write-Host ("wrote {0} ({1:N0} MB)" -f $zip, ((Get-Item $zip).Length / 1MB))

if ($NoInstaller) { Write-Host "Skipping the installer (-NoInstaller)."; exit 0 }

$makensis = Get-Command makensis -ErrorAction SilentlyContinue
if (-not $makensis) {
    $nsisPaths = @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe")
    $nsisPath = $nsisPaths | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if ($nsisPath) { $makensis = Get-Command $nsisPath }
}
if (-not $makensis) {
    if ($RequireInstaller) { throw 'NSIS is required for a release; no installer was produced' }
    # Not a failure. The portable folder and the zip above are a complete, working build; the
    # installer is the one artefact that needs a tool this machine does not have, and saying so
    # plainly beats failing a release build over a packaging nicety.
    Write-Host "makensis is not on PATH, so no installer was built." -ForegroundColor Yellow
    Write-Host "Install NSIS (https://nsis.sourceforge.io, zlib licence) and run this again for one."
    exit 0
}

Write-Host "== Building the installer ==" -ForegroundColor Cyan
$nsi = Join-Path $PSScriptRoot "owlwhisp.nsi"
& $makensis.Source /INPUTCHARSET UTF8 "/DVERSION=$version" "/DSTAGE=$stage" "/DOUTFILE=$root\dist\$name-setup.exe" "/DSIGN_MODE=$SigningMode" "/DSIGN_SCRIPT=$signScript" $nsi
if ($LASTEXITCODE -ne 0) { throw "makensis exited with $LASTEXITCODE" }
& $signScript -FilePath "$root\dist\$name-setup.exe" -Mode $SigningMode
Write-Host "wrote dist\$name-setup.exe"
