# Package LocalWisper for Windows into a portable folder, a zip, and (where the tool is present)
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
#   LocalWisper-<version>-<target>\        the portable folder; run the exe in place
#   LocalWisper-<version>-<target>.zip     the same, zipped
#   LocalWisper-<version>-<target>-setup.exe  only when makensis is on PATH

param(
    [string]$Target = "aarch64-pc-windows-msvc",
    [switch]$NoInstaller
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root

# The version is read from the workspace manifest rather than typed here, so a release cannot ship
# a folder named after the wrong one.
$manifest = Get-Content (Join-Path $root "Cargo.toml") -Raw
if ($manifest -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') {
    throw "could not read version from the workspace Cargo.toml"
}
$version = $Matches[1]

$exe = Join-Path $root "target\$Target\release\localwisper-gui.exe"
if (-not (Test-Path $exe)) {
    # Fall back to a host-target build, which is what a plain `cargo build --release` produces.
    $exe = Join-Path $root "target\release\localwisper-gui.exe"
}
if (-not (Test-Path $exe)) {
    throw "localwisper-gui.exe was not found. Build it first: cargo build -p lw-gui --release"
}

$runtime = Join-Path $root "runtime\win-arm64"
if (-not (Test-Path $runtime)) {
    throw "runtime\win-arm64 is missing. Fetch it first: scripts\runtime\fetch-runtime.ps1"
}

$name = "LocalWisper-$version-$Target"
$stage = Join-Path $root "dist\$name"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Write-Host "== Staging $name ==" -ForegroundColor Cyan
Copy-Item $exe (Join-Path $stage "LocalWisper.exe")

# The runtime keeps its own folder, because that is where the application looks for it: the loader
# resolves `runtime\win-arm64` relative to the executable.
$runtimeDest = Join-Path $stage "runtime\win-arm64"
New-Item -ItemType Directory -Force -Path $runtimeDest | Out-Null
Copy-Item -Recurse -Force (Join-Path $runtime "*") $runtimeDest

# Licences travel with the binaries they cover. This is not decoration: the ONNX Runtime and
# Qualcomm files are redistributed under terms that require their notices, and `option-ext` is
# MPL-2.0 (see docs\licenses.md).
Copy-Item (Join-Path $root "LICENSE") $stage -ErrorAction SilentlyContinue
Copy-Item (Join-Path $root "docs\licenses.md") (Join-Path $stage "LICENSES.md")

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
    # Not a failure. The portable folder and the zip above are a complete, working build; the
    # installer is the one artefact that needs a tool this machine does not have, and saying so
    # plainly beats failing a release build over a packaging nicety.
    Write-Host "makensis is not on PATH, so no installer was built." -ForegroundColor Yellow
    Write-Host "Install NSIS (https://nsis.sourceforge.io, zlib licence) and run this again for one."
    exit 0
}

Write-Host "== Building the installer ==" -ForegroundColor Cyan
$nsi = Join-Path $PSScriptRoot "localwisper.nsi"
& $makensis.Source "/DVERSION=$version" "/DSTAGE=$stage" "/DOUTFILE=$root\dist\$name-setup.exe" $nsi
if ($LASTEXITCODE -ne 0) { throw "makensis exited with $LASTEXITCODE" }
Write-Host "wrote dist\$name-setup.exe"
