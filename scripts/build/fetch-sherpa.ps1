# Stage the sherpa-onnx native libraries for a **GPL-free** build of the optional `sherpa` engine,
# and print the environment variable that points the build at them.
#
# Why this script exists
# ----------------------
# The `sherpa-onnx-sys` crate downloads a prebuilt archive during its build. The archive it picks
# by default statically links **espeak-ng (GPL-3.0-or-later)** and piper_phonemize, which exist
# only for sherpa-onnx's text-to-speech features -- features LocalWisper never calls. Shipping
# them would put GPL-3.0 code in the same binary as the proprietary Qualcomm QNN runtime, and
# those two licences cannot both bind one work. See docs/licenses.md.
#
# sherpa-onnx publishes `-no-tts` archives that omit espeak-ng, piper_phonemize and ucd. Every
# remaining component is permissive (Apache-2.0 / BSD-3-Clause / MIT). The crate's build script,
# however, emits `-l static=espeak-ng`, `-l piper_phonemize` and `-l ucd` unconditionally, so the
# link fails against a no-tts archive with "could not find native static library".
#
# This script therefore fetches the no-tts archive and generates three **empty** static libraries
# with those names. Nothing references their symbols -- the no-tts build of sherpa-onnx-core was
# compiled without TTS -- so the link succeeds and no TTS code of any licence enters the binary.
# The stubs satisfy a spurious flag; they do not stand in for anything.
#
# Usage:
#   pwsh -File scripts\build\fetch-sherpa.ps1
#   $env:SHERPA_ONNX_LIB_DIR = (pwsh -File scripts\build\fetch-sherpa.ps1 -Quiet)
#   cargo build --release -p lw-cli --features sherpa

param(
    [string]$Version = "1.13.6",
    [ValidateSet("win-arm64", "win-x64", "osx-arm64", "linux-x64", "")]
    [string]$Platform = "",
    # Print only the library directory, for capture into an environment variable.
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"

if (-not $Platform) {
    $arch = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq "Arm64") { "arm64" } else { "x64" }
    $Platform = if ($IsMacOS) { "osx-$arch" } elseif ($IsLinux) { "linux-$arch" } else { "win-$arch" }
}

$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$dest = Join-Path $root "third_party\sherpa-onnx\$Platform"
New-Item -ItemType Directory -Force -Path $dest | Out-Null

# Archive naming differs per platform; `-lib` variants carry only the libraries and headers.
$name = switch -Wildcard ($Platform) {
    "win-*"   { "sherpa-onnx-v$Version-$Platform-static-MT-Release-no-tts-lib" }
    "osx-*"   { "sherpa-onnx-v$Version-$Platform-static-no-tts-lib" }
    "linux-*" { "sherpa-onnx-v$Version-$Platform-static-no-tts-lib" }
}
$archive = "$name.tar.bz2"
$libDir = Join-Path $dest "$name\lib"

if (-not (Test-Path $libDir)) {
    $url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v$Version/$archive"
    $tmp = Join-Path $dest $archive
    if (-not $Quiet) { Write-Host "Downloading $url" -ForegroundColor Cyan }
    Invoke-WebRequest -Uri $url -OutFile $tmp
    if (-not $Quiet) { Write-Host "Extracting" }
    tar -xjf $tmp -C $dest
    Remove-Item $tmp -Force
}
if (-not (Test-Path $libDir)) { throw "expected libraries at $libDir" }

# Fail loudly if a future archive starts shipping the TTS chain again: the whole point of using
# the no-tts build is that these must not be here.
foreach ($f in @("espeak-ng", "piper_phonemize", "ucd")) {
    $real = Get-ChildItem $libDir -Filter "*$f*" -ErrorAction SilentlyContinue |
        Where-Object { $_.Length -gt 10KB }
    if ($real) {
        throw "the '$Platform' no-tts archive unexpectedly contains a real $f library ($($real[0].Name), $($real[0].Length) bytes). Refusing: that is the GPL-3.0 component this script exists to avoid."
    }
}

# Generate the empty stand-ins the crate's link flags demand.
Push-Location $libDir
try {
    $stubSrc = "lw_sherpa_stub.c"
    if (-not (Test-Path $stubSrc)) {
        # One unused symbol so the object file is valid on every toolchain.
        "static int lw_sherpa_stub_unused;" | Out-File $stubSrc -Encoding ascii
    }
    if ($Platform -like "win-*") {
        if (-not (Test-Path "lw_sherpa_stub.obj")) { & cl.exe /nologo /c $stubSrc | Out-Null }
        foreach ($f in @("espeak-ng", "piper_phonemize", "ucd")) {
            if (-not (Test-Path "$f.lib")) { & lib.exe /nologo "/OUT:$f.lib" lw_sherpa_stub.obj | Out-Null }
        }
    } else {
        if (-not (Test-Path "lw_sherpa_stub.o")) { & cc -c $stubSrc -o lw_sherpa_stub.o }
        foreach ($f in @("espeak-ng", "piper_phonemize", "ucd")) {
            if (-not (Test-Path "lib$f.a")) { & ar rcs "lib$f.a" lw_sherpa_stub.o }
        }
    }
} finally {
    Pop-Location
}

if ($Quiet) {
    Write-Output $libDir
} else {
    Write-Host "`nGPL-free sherpa-onnx libraries staged." -ForegroundColor Green
    Get-ChildItem $libDir | Where-Object { $_.Extension -in ".lib", ".a" } | Select-Object Length, Name | Format-Table -AutoSize
    Write-Host "Build with:" -ForegroundColor Cyan
    Write-Host "  `$env:SHERPA_ONNX_LIB_DIR = `"$libDir`""
    Write-Host "  cargo build --release -p lw-cli --features sherpa"
    Write-Host "`nThe three zero-byte-content libraries above are deliberate stubs; see the header of this script."
}
