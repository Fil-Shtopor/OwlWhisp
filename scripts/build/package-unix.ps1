# Package OwlWhisp into a user-installable archive on macOS or Linux.
#
# macOS receives a self-contained .app bundle; Linux receives a portable tarball.
# Both contain the application, the platform-specific ONNX Runtime files, and licences.
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("osx-arm64", "osx-x64", "linux-x64", "linux-arm64")]
    [string]$Platform,
    [Parameter(Mandatory = $true)]
    [string]$Target
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root
$manifest = Get-Content "$root/Cargo.toml" -Raw
if ($manifest -notmatch '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"') { throw "could not read version" }
$version = $Matches[1]
$binary = Join-Path $root "target/$Target/release/owlwhisp"
if ($IsWindows) { $binary += ".exe" }
if (-not (Test-Path $binary)) { throw "owlwhisp was not found at $binary" }
$runtime = Join-Path $root "runtime/$Platform"
if (-not (Test-Path $runtime)) { throw "runtime/$Platform is missing; run fetch-runtime.ps1 first" }
$runtimeLibrary = if ($Platform -like "osx-*") { "libonnxruntime.dylib" } else { "libonnxruntime.so" }
if (-not (Test-Path (Join-Path $runtime $runtimeLibrary))) {
    throw "runtime/$Platform/$runtimeLibrary is missing; fetch ONNX Runtime before packaging."
}
$manifests = Join-Path $root "models/manifests"
if (-not (Test-Path $manifests)) {
    throw "models/manifests is missing; cannot package verified model downloads."
}
$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$name = "OwlWhisp-$version-$Platform"

if ($Platform -like "osx-*") {
    $app = Join-Path $dist "$name.app"
    Remove-Item -Recurse -Force $app -ErrorAction SilentlyContinue
    $macos = Join-Path $app "Contents/MacOS"
    $resources = Join-Path $app "Contents/Resources"
    New-Item -ItemType Directory -Force -Path $macos, $resources, "$macos/runtime/$Platform", "$macos/models/manifests" | Out-Null
    Copy-Item $binary (Join-Path $macos "OwlWhisp")
    Copy-Item -Recurse -Force "$runtime/*" "$macos/runtime/$Platform"
    # The weights remain in per-user application data; only their signed manifests ship in the
    # bundle, so the first-run Parakeet download can be verified and resumed.
    Copy-Item -Recurse -Force "$manifests/*" "$macos/models/manifests"
    Copy-Item assets/icons/icon-256.png (Join-Path $resources "OwlWhisp.png")
    Copy-Item LICENSE (Join-Path $resources "LICENSE")
    Copy-Item docs/licenses.md (Join-Path $resources "LICENSES.md")
    @"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDisplayName</key><string>OwlWhisp</string>
  <key>CFBundleExecutable</key><string>OwlWhisp</string>
  <key>CFBundleIdentifier</key><string>ai.owlwhisp.app</string>
  <key>CFBundleName</key><string>OwlWhisp</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
</dict></plist>
"@ | Set-Content (Join-Path $app "Contents/Info.plist") -NoNewline
    Compress-Archive -Path $app -DestinationPath (Join-Path $dist "$name-macos.zip") -Force
} else {
    $stage = Join-Path $dist $name
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path "$stage/runtime", "$stage/models/manifests" | Out-Null
    Copy-Item $binary (Join-Path $stage "owlwhisp")
    Copy-Item -Recurse -Force "$runtime/*" "$stage/runtime"
    Copy-Item -Recurse -Force "$manifests/*" "$stage/models/manifests"
    Copy-Item LICENSE "$stage/LICENSE"
    Copy-Item docs/licenses.md "$stage/LICENSES.md"
    tar -C $dist -czf "$dist/$name.tar.gz" $name
}
