# Prepare an existing signing identity. This never creates a certificate or cloud account.
param([string]$Mode = $env:OWLWHISP_SIGNING_MODE)
$ErrorActionPreference = 'Stop'
if (-not $Mode -or $Mode -eq 'None') { return }
if (-not $env:GITHUB_ENV) { throw 'This setup script is intended for GitHub Actions' }
if ($Mode -eq 'Certificate') {
    # Use a signing runner with its certificate/HSM already provisioned.
    if ($env:OWLWHISP_SIGNING_CERTIFICATE -notmatch '^[a-fA-F0-9]{40}$') { throw 'Missing signing certificate thumbprint' }
    $cert = Get-Item -LiteralPath "Cert:/CurrentUser/My/$env:OWLWHISP_SIGNING_CERTIFICATE"
    if (-not $cert.HasPrivateKey) { throw 'Signing runner cannot access the certificate private key/HSM' }
    "OWLWHISP_SIGNING_CERTIFICATE=$env:OWLWHISP_SIGNING_CERTIFICATE" | Out-File $env:GITHUB_ENV -Append -Encoding utf8
    return
}
if ($Mode -ne 'Azure') { throw 'Unknown signing mode' }
foreach ($name in @('OWLWHISP_SIGNING_ENDPOINT', 'OWLWHISP_SIGNING_ACCOUNT', 'OWLWHISP_SIGNING_PROFILE')) {
    if (-not [Environment]::GetEnvironmentVariable($name)) { throw "Missing repository variable $name" }
}
if ($env:OWLWHISP_SIGNING_ENDPOINT -notmatch '^https://[a-z0-9]+\.codesigning\.azure\.net/?$') { throw 'Invalid Azure signing endpoint' }
# Official Microsoft package, pinned rather than implicitly changing on every release.
$version = '1.0.128'
$dir = Join-Path $env:RUNNER_TEMP 'owlwhisp-artifact-signing'
New-Item -ItemType Directory -Force $dir | Out-Null
$package = Join-Path $dir 'client.zip'
Invoke-WebRequest "https://api.nuget.org/v3-flatcontainer/microsoft.artifactsigning.client/$version/microsoft.artifactsigning.client.$version.nupkg" -OutFile $package
Expand-Archive -LiteralPath $package -DestinationPath (Join-Path $dir 'client') -Force
$dlib = Join-Path $dir 'client/bin/x64/Azure.CodeSigning.Dlib.dll'
if (-not (Test-Path -LiteralPath $dlib)) { throw 'Microsoft signing package does not contain the x64 dlib' }
$metadata = Join-Path $dir 'metadata.json'
@{
    Endpoint = $env:OWLWHISP_SIGNING_ENDPOINT
    CodeSigningAccountName = $env:OWLWHISP_SIGNING_ACCOUNT
    CertificateProfileName = $env:OWLWHISP_SIGNING_PROFILE
    CorrelationId = "$env:GITHUB_RUN_ID-$env:GITHUB_JOB"
    # azure/login supplies AzureCliCredential, without persisting a signing key.
    ExcludeCredentials = @('EnvironmentCredential', 'ManagedIdentityCredential', 'WorkloadIdentityCredential',
        'SharedTokenCacheCredential', 'VisualStudioCredential', 'VisualStudioCodeCredential',
        'AzurePowerShellCredential', 'AzureDeveloperCliCredential', 'InteractiveBrowserCredential')
} | ConvertTo-Json | Set-Content -LiteralPath $metadata -Encoding utf8
"OWLWHISP_SIGNING_DLIB=$dlib" | Out-File $env:GITHUB_ENV -Append -Encoding utf8
"OWLWHISP_SIGNING_METADATA=$metadata" | Out-File $env:GITHUB_ENV -Append -Encoding utf8
