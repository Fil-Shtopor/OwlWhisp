# Sign with an existing trusted certificate/HSM or an already authenticated cloud signer.
# No certificate/key is generated or exported by this script.
param(
    [Parameter(Mandatory = $true)][string]$FilePath,
    [ValidateSet('None', 'Certificate', 'Azure')][string]$Mode = 'None',
    [string]$CertificateThumbprint = $env:OWLWHISP_SIGNING_CERTIFICATE,
    [string]$SignToolPath = $env:OWLWHISP_SIGNTOOL,
    [string]$DlibPath = $env:OWLWHISP_SIGNING_DLIB,
    [string]$MetadataPath = $env:OWLWHISP_SIGNING_METADATA,
    [string]$TimestampUrl = 'http://timestamp.digicert.com'
)
$ErrorActionPreference = 'Stop'
if ($Mode -eq 'None') { return }
$resolvedFile = (Resolve-Path -LiteralPath $FilePath).Path
if (-not $SignToolPath) {
    $sdkBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
    $SignToolPath = Get-ChildItem -LiteralPath $sdkBin -Directory | Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64/signtool.exe' } |
        Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
if (-not $SignToolPath -or -not (Test-Path -LiteralPath $SignToolPath)) { throw 'A Windows SDK SignTool is required for signing' }
$signArguments = @('sign', '/fd', 'SHA256', '/td', 'SHA256', '/d', 'OwlWhisp')
if ($Mode -eq 'Certificate') {
    if ($CertificateThumbprint -notmatch '^[a-fA-F0-9]{40}$') { throw 'Set OWLWHISP_SIGNING_CERTIFICATE to a code-signing certificate thumbprint' }
    $certificate = Get-Item -LiteralPath "Cert:/CurrentUser/My/$CertificateThumbprint"
    if (-not $certificate.HasPrivateKey) { throw 'The signing certificate has no accessible private key/HSM' }
    $signArguments += @('/sha1', $CertificateThumbprint, '/tr', $TimestampUrl)
} else {
    if (-not (Test-Path -LiteralPath $DlibPath) -or -not (Test-Path -LiteralPath $MetadataPath)) {
        throw 'Azure signing requires OWLWHISP_SIGNING_DLIB and OWLWHISP_SIGNING_METADATA plus an authenticated identity'
    }
    $signArguments += @('/dlib', $DlibPath, '/dmdf', $MetadataPath, '/tr', 'http://timestamp.acs.microsoft.com')
}
& $SignToolPath @signArguments $resolvedFile
if ($LASTEXITCODE -ne 0) { throw "SignTool failed for $resolvedFile" }
& $SignToolPath verify /pa /all $resolvedFile
if ($LASTEXITCODE -ne 0) { throw "Windows does not trust the signature on $resolvedFile" }
$signature = Get-AuthenticodeSignature -LiteralPath $resolvedFile
if ($signature.Status -ne 'Valid' -or -not $signature.TimeStamperCertificate) {
    throw 'Signing requires both a trusted Authenticode signature and a timestamp'
}
Write-Host "Verified signed file: $([IO.Path]::GetFileName($resolvedFile)); publisher: $($signature.SignerCertificate.Subject)"
