# Windows publisher and signing

OwlWhisp's EXE, installer, uninstall entry and shortcuts identify the product as
OwlWhisp and the developer as Fil-Shtopor. These metadata fields do not establish
a verified Windows publisher. Windows obtains that name from a trusted
Authenticode certificate's subject, after identity validation by the provider.
The subject may be a legal personal or organisation name rather than a GitHub
handle. A self-signed certificate does not solve the public distribution warning.

Signing is prepared but **not enabled**: no trusted certificate or signing account
has been supplied. Until then, released Windows packages remain unsigned.
Even trusted signing does not guarantee that a new binary immediately avoids
every SmartScreen reputation prompt. See Microsoft's
[code signing options](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options).

## Existing certificate or HSM

Install the provider's certificate and HSM/token middleware on a trusted signing
machine. It must appear in `Cert:\CurrentUser\My` with an accessible private key.
Install Windows SDK SignTool. No private key needs to be exported into this repo.

```powershell
$env:OWLWHISP_SIGNING_CERTIFICATE = '<40-character certificate thumbprint>'
pwsh scripts/build/package-windows.ps1 -Target x86_64-pc-windows-msvc -Platform win-x64 -SigningMode Certificate -RequireSigning -RequireInstaller
```

For CI, use a provisioned Windows signing runner and configure its runner labels
in the release matrix. Set repository variables `OWLWHISP_SIGNING_MODE=Certificate`
and `OWLWHISP_SIGNING_CERTIFICATE`. The default GitHub-hosted runners have no
certificate/HSM; this mode fails instead of silently producing an unsigned file.

## Azure Artifact Signing with GitHub OIDC

This optional integration uses the provider's existing validated account and
public-trust certificate profile. Check the provider's eligibility before
creating an account. Follow Microsoft's
[setup and identity requirements](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart)
and [SignTool integration](https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-signing-integrations).

Configure an Entra application with a GitHub federated credential restricted to
this repository's release tag refs, and assign its identity the **Artifact Signing
Certificate Profile Signer** role on the signing profile. Set these repository
variables; there is no client secret or private signing key in GitHub:

A standard federated credential uses an exact subject, for example
`repo:Fil-Shtopor/OwlWhisp:ref:refs/tags/v0.1.3`. Provision a credential for each
release tag (and a separate branch subject if allowing workflow dispatch).
A literal wildcard in a standard subject does not match future tags.

| Variable | Value |
| --- | --- |
| `OWLWHISP_SIGNING_MODE` | `Azure` |
| `OWLWHISP_AZURE_CLIENT_ID` | Entra application's client ID |
| `OWLWHISP_AZURE_TENANT_ID` | Tenant ID |
| `OWLWHISP_AZURE_SUBSCRIPTION_ID` | Subscription ID |
| `OWLWHISP_SIGNING_ENDPOINT` | Profile's regional HTTPS endpoint |
| `OWLWHISP_SIGNING_ACCOUNT` | Existing signing account name |
| `OWLWHISP_SIGNING_PROFILE` | Existing public-trust certificate profile name |

The release workflow authenticates with OIDC, prepares the official Microsoft
Artifact Signing client and x64 .NET 8 tools on both Windows build hosts, and
uses the same signer for the EXE, NSIS uninstaller and final installer.
Local cloud signing also works with an authenticated Azure identity and
`OWLWHISP_SIGNING_DLIB`/`OWLWHISP_SIGNING_METADATA` pointing to the provider's
x64 dlib and metadata JSON; pass `-SigningMode Azure -RequireSigning`.

Every signing step requires SHA-256 Authenticode, a timestamp, `signtool verify
/pa /all` and a valid Windows signature. Any failure blocks packaging/publication.
Checksums are generated **after** signing. In-app updates retain Internet download
provenance and open the installer through Windows ShellExecute. A signed app
also refuses an unsigned update or a different certificate subject; an unsigned
legacy app can migrate to the first signed release.
