# Official ORT releases no longer publish Intel macOS binaries. Build the same pinned source API.
param([string]$Version = '1.28.1')
$ErrorActionPreference = 'Stop'
if (-not $IsMacOS -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
    throw 'The Intel macOS runtime must be built on a native Intel Mac'
}
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$source = Join-Path $root "third_party/onnxruntime-osx-x64-$Version"
$dest = Join-Path $root 'runtime/osx-x64'
New-Item -ItemType Directory -Force -Path $dest | Out-Null
if ((Test-Path "$dest/libonnxruntime.dylib") -and (Test-Path "$dest/onnxruntime-LICENSE") -and (Test-Path "$dest/onnxruntime-ThirdPartyNotices.txt")) { return }
if (-not (Test-Path (Join-Path $dest 'libonnxruntime.dylib'))) {
    if (-not (Test-Path (Join-Path $source '.git'))) {
        git clone --depth 1 --branch "v$Version" --recurse-submodules --shallow-submodules https://github.com/microsoft/onnxruntime.git $source
        if ($LASTEXITCODE -ne 0) { throw 'Failed to fetch pinned ONNX Runtime source' }
    }
    if ($Version -eq '1.28.1' -and (git -C $source rev-parse HEAD) -ne '5181af9bef60f46194ec30506855d74b6e6a96ff') {
        throw 'The ORT source does not match the reviewed 1.28.1 commit'
    }
    python3 "$source/tools/ci_build/build.py" --build_dir "$source/build" --config Release --update --build --build_shared_lib --skip_tests --parallel 3 --compile_no_warning_as_error --cmake_extra_defines CMAKE_OSX_DEPLOYMENT_TARGET=13.3
    if ($LASTEXITCODE -ne 0) { throw 'Intel macOS ONNX Runtime build failed' }
    $libraries = @(Get-ChildItem "$source/build/Release" -File -Filter 'libonnxruntime*.dylib')
    if (-not $libraries) { throw 'ORT build produced no dylib' }
    $libraries | ForEach-Object { Copy-Item $_.FullName $dest -Force }
    if (-not (Test-Path (Join-Path $dest 'libonnxruntime.dylib'))) { throw 'ORT build produced no loader library' }
}
Copy-Item "$source/LICENSE" "$dest/onnxruntime-LICENSE" -Force
Copy-Item "$source/ThirdPartyNotices.txt" "$dest/onnxruntime-ThirdPartyNotices.txt" -Force
