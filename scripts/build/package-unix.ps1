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

function Reset-Stage([string]$path) {
    $resolved = [IO.Path]::GetFullPath($path)
    $boundary = [IO.Path]::GetFullPath($dist) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($boundary, [StringComparison]::Ordinal)) { throw 'Stage is outside dist' }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}

if ($Platform -like "osx-*") {
    $app = Join-Path $dist "$name.app"
    Reset-Stage $app
    $macos = Join-Path $app "Contents/MacOS"
    $resources = Join-Path $app "Contents/Resources"
    New-Item -ItemType Directory -Force -Path $macos, $resources, "$macos/runtime/$Platform", "$macos/models/manifests" | Out-Null
    Copy-Item $binary (Join-Path $macos "OwlWhisp")
    chmod +x (Join-Path $macos "OwlWhisp")
    if ($LASTEXITCODE -ne 0) { throw 'Could not set executable permission' }
    Copy-Item -Recurse -Force "$runtime/*" "$macos/runtime/$Platform"
    # The weights remain in per-user application data; only their signed manifests ship in the
    # bundle, so the first-run Parakeet download can be verified and resumed.
    Copy-Item -Recurse -Force "$manifests/*" "$macos/models/manifests"
    New-Item -ItemType Directory -Force -Path "$macos/benchmark/audio" | Out-Null
    Copy-Item -Recurse -Force "$root/tests/fixtures/audio/*" "$macos/benchmark/audio"
    Copy-Item assets/icons/icon-256.png (Join-Path $resources "OwlWhisp.png")
    Copy-Item LICENSE (Join-Path $resources "LICENSE")
    Copy-Item docs/licenses.md (Join-Path $resources "LICENSES.md")
    Copy-Item THIRD_PARTY_NOTICES.md (Join-Path $resources 'THIRD_PARTY_NOTICES.md')
    @{
        version = $version; target = $Target; platform = $Platform
        commit = (git rev-parse HEAD); sherpa = ($Platform -ne 'linux-arm64')
    } | ConvertTo-Json | Set-Content (Join-Path $resources 'BUILD_INFO.json')
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
  <key>LSMinimumSystemVersion</key><string>13.3</string>
</dict></plist>
"@ | Set-Content (Join-Path $app "Contents/Info.plist") -NoNewline
    # ditto preserves Mach-O executable modes and macOS bundle metadata; Compress-Archive does not.
    $zip = Join-Path $dist "$name-macos.zip"
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip }
    ditto -c -k --sequesterRsrc --keepParent $app $zip
    if ($LASTEXITCODE -ne 0) { throw 'macOS archive creation failed' }
} else {
    $stage = Join-Path $dist $name
    Reset-Stage $stage
    New-Item -ItemType Directory -Force -Path "$stage/runtime", "$stage/models/manifests" | Out-Null
    Copy-Item $binary (Join-Path $stage "owlwhisp")
    chmod +x (Join-Path $stage "owlwhisp")
    if ($LASTEXITCODE -ne 0) { throw 'Could not set executable permission' }
    Copy-Item -Recurse -Force "$runtime/*" "$stage/runtime"
    Copy-Item -Recurse -Force "$manifests/*" "$stage/models/manifests"
    New-Item -ItemType Directory -Force -Path "$stage/benchmark/audio" | Out-Null
    Copy-Item -Recurse -Force "$root/tests/fixtures/audio/*" "$stage/benchmark/audio"
    Copy-Item LICENSE "$stage/LICENSE"
    Copy-Item docs/licenses.md "$stage/LICENSES.md"
    Copy-Item THIRD_PARTY_NOTICES.md "$stage/THIRD_PARTY_NOTICES.md"
    @{
        version = $version; target = $Target; platform = $Platform
        commit = (git rev-parse HEAD); sherpa = ($Platform -ne 'linux-arm64')
    } | ConvertTo-Json | Set-Content "$stage/BUILD_INFO.json"
    tar -C $dist -czf "$dist/$name.tar.gz" $name
    if ($LASTEXITCODE -ne 0) { throw 'Linux archive creation failed' }
}
